use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::irq_lock::IrqMutex as Mutex;

use crate::capability::{Capability, CapabilitySet};
use crate::config::{
    IPC_QUEUE_DEPTH as QUEUE_DEPTH, MAX_IPC_ENDPOINTS as MAX_ENDPOINTS,
    MAX_IPC_HANDLES_PER_PROCESS as MAX_HANDLES, MAX_IPC_OBJECTS as MAX_OBJECTS,
    MAX_IPC_SERVICES as MAX_SERVICES, MAX_IPC_SERVICE_NAME as MAX_SERVICE_NAME,
    MAX_SHARED_MEMORY_MAPPINGS as MAX_SHM_MAPPINGS, MAX_SHARED_MEMORY_OBJECTS as MAX_SHM_OBJECTS,
    MAX_SHARED_MEMORY_PAGES as MAX_SHM_PAGES,
};
use crate::paging;
use crate::task::{self, TaskId};
use crate::wovenguard::{self, LineageId, ServiceClass};

pub use crate::config::MAX_MESSAGE_SIZE;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Message {
    pub sender: u64,
    length: usize,
    bytes: [u8; MAX_MESSAGE_SIZE],
    transferred_handle: Option<Handle>,
}

impl Message {
    const EMPTY: Self = Self {
        sender: 0,
        length: 0,
        bytes: [0; MAX_MESSAGE_SIZE],
        transferred_handle: None,
    };

    pub fn payload(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// Stage 8.5: receiver-local handle installed atomically while dequeuing a
    /// capability-bearing message. Ordinary messages return `None`.
    pub const fn transferred_handle(&self) -> Option<Handle> {
        self.transferred_handle
    }
}

/// Stage 8.1 process-local IPC handle.
///
/// The low byte stores the handle-table slot plus one; the upper bits carry a
/// generation. Reusing a closed slot therefore produces a different handle and
/// prevents a stale userspace value from silently naming a new object.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Handle(u32);

impl Handle {
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    const fn from_parts(slot: usize, generation: u32) -> Self {
        Self((generation << 8) | ((slot as u32) + 1))
    }

    fn slot(self) -> Option<usize> {
        let encoded = (self.0 & 0xff) as usize;
        if encoded == 0 {
            return None;
        }
        let slot = encoded - 1;
        (slot < MAX_HANDLES).then_some(slot)
    }

    const fn generation(self) -> u32 {
        self.0 >> 8
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EndpointObjectId(u64);

impl EndpointObjectId {
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Rights attached to a process-local handle. Stage 8.1 records and validates
/// them; later IPC stages will use these bits to gate send/receive/transfer.
pub struct HandleRights;

impl HandleRights {
    pub const SEND: u8 = 1 << 0;
    pub const RECEIVE: u8 = 1 << 1;
    pub const TRANSFER: u8 = 1 << 2;
    pub const INSPECT: u8 = 1 << 3;
    pub const SHM_READ: u8 = 1 << 4;
    pub const SHM_WRITE: u8 = 1 << 5;
    pub const SHM_MAP: u8 = 1 << 6;
    /// Stage 8.6 authority to publish an endpoint in the global service registry.
    pub const PUBLISH_SERVICE: u8 = 1 << 7;
    pub const ENDPOINT_OWNER: u8 =
        Self::SEND | Self::RECEIVE | Self::TRANSFER | Self::INSPECT | Self::PUBLISH_SERVICE;
    /// Rights that may be delegated by service discovery. RECEIVE and
    /// PUBLISH_SERVICE stay server-side so discovery cannot mint another server.
    pub const SERVICE_CLIENT: u8 = Self::SEND | Self::TRANSFER | Self::INSPECT;
    pub const SHARED_MEMORY_OWNER: u8 =
        Self::TRANSFER | Self::INSPECT | Self::SHM_READ | Self::SHM_WRITE | Self::SHM_MAP;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HandleObjectKind {
    Endpoint,
    SharedMemory,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HandleInfo {
    pub object_id: EndpointObjectId,
    pub owner: u64,
    pub rights: u8,
    pub kind: HandleObjectKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SharedMemoryInfo {
    pub object_id: EndpointObjectId,
    pub owner: u64,
    pub rights: u8,
    pub page_count: usize,
    pub mapping_count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ServiceInfo {
    pub object_id: EndpointObjectId,
    pub owner: u64,
    pub client_rights: u8,
    pub name_length: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EndpointExists,
    NoEndpoint,
    RegistryFull,
    QueueFull,
    QueueEmpty,
    MessageTooLarge,
    NoHandleSpace,
    HandleTableFull,
    ObjectTableFull,
    InvalidHandle,
    AccessDenied,
    WaiterTableFull,
    InvalidSize,
    MappingTableFull,
    AlreadyMapped,
    NotMapped,
    MappingFailed,
    MappingBusy,
    InvalidServiceName,
    ServiceExists,
    ServiceNotFound,
    ServiceTableFull,
}

/// Legacy PID-addressed queue endpoint kept intact through Stage 8.1 so the
/// existing MessageSend/MessageReceive ABI continues to work.
#[derive(Clone, Copy)]
struct LegacyEndpoint {
    owner: u64,
    messages: [Message; QUEUE_DEPTH],
    length: usize,
}

impl LegacyEndpoint {
    const EMPTY: Self = Self {
        owner: 0,
        messages: [Message::EMPTY; QUEUE_DEPTH],
        length: 0,
    };

    fn enqueue(&mut self, message: Message) -> Result<(), Error> {
        if self.length == QUEUE_DEPTH {
            return Err(Error::QueueFull);
        }
        self.messages[self.length] = message;
        self.length += 1;
        Ok(())
    }

    fn dequeue(&mut self) -> Result<Message, Error> {
        if self.length == 0 {
            return Err(Error::QueueEmpty);
        }
        let message = self.messages[0];
        self.messages.copy_within(1..self.length, 0);
        self.length -= 1;
        self.messages[self.length] = Message::EMPTY;
        Ok(message)
    }
}

const MAX_WAITERS: usize = MAX_HANDLES;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Waiter {
    owner: u64,
    handle: Handle,
    task: TaskId,
}

fn push_waiter(slots: &mut [Option<Waiter>; MAX_WAITERS], waiter: Waiter) -> Result<(), Error> {
    if slots.iter().flatten().any(|current| *current == waiter) {
        return Ok(());
    }
    let slot = slots
        .iter_mut()
        .find(|slot| slot.is_none())
        .ok_or(Error::WaiterTableFull)?;
    *slot = Some(waiter);
    Ok(())
}

fn take_waiter(slots: &mut [Option<Waiter>; MAX_WAITERS]) -> Option<Waiter> {
    let waiter = slots[0]?;
    slots.copy_within(1..MAX_WAITERS, 0);
    slots[MAX_WAITERS - 1] = None;
    Some(waiter)
}

fn remove_waiters_for_handle(
    slots: &mut [Option<Waiter>; MAX_WAITERS],
    owner: u64,
    handle: Handle,
) -> [Option<TaskId>; MAX_WAITERS] {
    let mut wake = [None; MAX_WAITERS];
    let mut wake_len = 0usize;
    let mut compact = [None; MAX_WAITERS];
    let mut compact_len = 0usize;
    for waiter in slots.iter().flatten().copied() {
        if waiter.owner == owner && waiter.handle == handle {
            if wake_len < MAX_WAITERS {
                wake[wake_len] = Some(waiter.task);
                wake_len += 1;
            }
        } else {
            compact[compact_len] = Some(waiter);
            compact_len += 1;
        }
    }
    *slots = compact;
    wake
}

fn remove_waiters_for_owner(slots: &mut [Option<Waiter>; MAX_WAITERS], owner: u64) {
    let mut compact = [None; MAX_WAITERS];
    let mut compact_len = 0usize;
    for waiter in slots.iter().flatten().copied() {
        if waiter.owner != owner {
            compact[compact_len] = Some(waiter);
            compact_len += 1;
        }
    }
    *slots = compact;
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TransferEscrow {
    object: ObjectRef,
    rights: u8,
    lineage: Option<LineageId>,
    service_class: Option<ServiceClass>,
}

#[derive(Clone, Copy)]
struct QueuedMessage {
    message: Message,
    transfer: Option<TransferEscrow>,
}

impl QueuedMessage {
    const EMPTY: Self = Self {
        message: Message::EMPTY,
        transfer: None,
    };
}

#[derive(Clone, Copy)]
struct EndpointObject {
    id: EndpointObjectId,
    owner: u64,
    references: u16,
    messages: [QueuedMessage; QUEUE_DEPTH],
    length: usize,
    receive_waiters: [Option<Waiter>; MAX_WAITERS],
    send_waiters: [Option<Waiter>; MAX_WAITERS],
    delegation_lineage: Option<LineageId>,
}

impl EndpointObject {
    fn enqueue(&mut self, message: QueuedMessage) -> Result<(), Error> {
        if self.length == QUEUE_DEPTH {
            return Err(Error::QueueFull);
        }
        self.messages[self.length] = message;
        self.length += 1;
        Ok(())
    }

    fn head(&self) -> Result<QueuedMessage, Error> {
        if self.length == 0 {
            return Err(Error::QueueEmpty);
        }
        Ok(self.messages[0])
    }

    fn dequeue(&mut self) -> Result<QueuedMessage, Error> {
        let message = self.head()?;
        self.messages.copy_within(1..self.length, 0);
        self.length -= 1;
        self.messages[self.length] = QueuedMessage::EMPTY;
        Ok(message)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ObjectRef {
    id: EndpointObjectId,
    kind: HandleObjectKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SharedMapping {
    owner: u64,
    space: paging::AddressSpace,
    start: u64,
}

#[derive(Clone, Copy)]
struct SharedMemoryObject {
    id: EndpointObjectId,
    owner: u64,
    references: u16,
    page_count: usize,
    frames: [u64; MAX_SHM_PAGES],
    mappings: [Option<SharedMapping>; MAX_SHM_MAPPINGS],
    delegation_lineage: Option<LineageId>,
}

#[derive(Clone, Copy)]
struct HandleEntry {
    object: ObjectRef,
    rights: u8,
    generation: u32,
    lineage: Option<LineageId>,
    service_class: Option<ServiceClass>,
}

#[derive(Clone, Copy)]
struct HandleSpace {
    owner: u64,
    entries: [Option<HandleEntry>; MAX_HANDLES],
    generations: [u32; MAX_HANDLES],
}

impl HandleSpace {
    const EMPTY: Self = Self {
        owner: 0,
        entries: [None; MAX_HANDLES],
        generations: [0; MAX_HANDLES],
    };

    fn allocate(
        &mut self,
        object: ObjectRef,
        rights: u8,
        lineage: Option<LineageId>,
    ) -> Result<Handle, Error> {
        self.allocate_tagged(object, rights, lineage, None)
    }

    fn allocate_tagged(
        &mut self,
        object: ObjectRef,
        rights: u8,
        lineage: Option<LineageId>,
        service_class: Option<ServiceClass>,
    ) -> Result<Handle, Error> {
        let slot = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(Error::HandleTableFull)?;
        let mut generation = self.generations[slot].wrapping_add(1) & 0x00ff_ffff;
        if generation == 0 {
            generation = 1;
        }
        self.generations[slot] = generation;
        self.entries[slot] = Some(HandleEntry {
            object,
            rights,
            generation,
            lineage,
            service_class,
        });
        Ok(Handle::from_parts(slot, generation))
    }

    fn lookup_raw(&self, handle: Handle) -> Result<HandleEntry, Error> {
        let slot = handle.slot().ok_or(Error::InvalidHandle)?;
        let entry = self.entries[slot].ok_or(Error::InvalidHandle)?;
        if entry.generation != handle.generation() {
            return Err(Error::InvalidHandle);
        }
        Ok(entry)
    }

    fn lookup(&self, handle: Handle) -> Result<HandleEntry, Error> {
        let entry = self.lookup_raw(handle)?;
        if entry
            .lineage
            .is_some_and(|lineage| !wovenguard::lineage_authorizes(lineage, Capability::Ipc))
        {
            return Err(Error::AccessDenied);
        }
        Ok(entry)
    }

    fn remove(&mut self, handle: Handle) -> Result<HandleEntry, Error> {
        let slot = handle.slot().ok_or(Error::InvalidHandle)?;
        let entry = self.lookup_raw(handle)?;
        self.entries[slot] = None;
        Ok(entry)
    }

    fn count(&self) -> usize {
        self.entries.iter().flatten().count()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ServiceName {
    length: u8,
    bytes: [u8; MAX_SERVICE_NAME],
}

impl ServiceName {
    fn parse(name: &[u8]) -> Result<Self, Error> {
        if name.is_empty() || name.len() > MAX_SERVICE_NAME {
            return Err(Error::InvalidServiceName);
        }
        // Keep the kernel namespace deterministic and shell/config friendly.
        // Hierarchical names such as `woven.fs` and `system.net-v1` are valid.
        if !name
            .iter()
            .copied()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(Error::InvalidServiceName);
        }
        let mut bytes = [0_u8; MAX_SERVICE_NAME];
        bytes[..name.len()].copy_from_slice(name);
        Ok(Self {
            length: name.len() as u8,
            bytes,
        })
    }

    fn matches(&self, name: &[u8]) -> bool {
        self.length as usize == name.len() && &self.bytes[..name.len()] == name
    }
}

#[derive(Clone, Copy)]
struct ServiceEntry {
    name: ServiceName,
    owner: u64,
    object: ObjectRef,
    client_rights: u8,
    lineage: Option<LineageId>,
    class: ServiceClass,
}

struct State {
    legacy_endpoints: [Option<LegacyEndpoint>; MAX_ENDPOINTS],
    objects: [Option<EndpointObject>; MAX_OBJECTS],
    shared_memory: [Option<SharedMemoryObject>; MAX_SHM_OBJECTS],
    services: [Option<ServiceEntry>; MAX_SERVICES],
    handle_spaces: [Option<HandleSpace>; MAX_ENDPOINTS],
    next_object_id: u64,
}

impl State {
    const fn new() -> Self {
        Self {
            legacy_endpoints: [None; MAX_ENDPOINTS],
            objects: [None; MAX_OBJECTS],
            shared_memory: [None; MAX_SHM_OBJECTS],
            services: [None; MAX_SERVICES],
            handle_spaces: [None; MAX_ENDPOINTS],
            next_object_id: 1,
        }
    }

    /// Registers the IPC namespace for a process. Stage 8.1 makes legacy queue
    /// registration and handle-space registration one atomic operation under
    /// the IPC lock, so a process can never exist with only half an IPC state.
    fn register(&mut self, owner: u64) -> Result<(), Error> {
        if self.legacy_endpoint(owner).is_some() || self.handle_space(owner).is_some() {
            return Err(Error::EndpointExists);
        }
        let endpoint_slot = self
            .legacy_endpoints
            .iter()
            .position(Option::is_none)
            .ok_or(Error::RegistryFull)?;
        let handle_slot = self
            .handle_spaces
            .iter()
            .position(Option::is_none)
            .ok_or(Error::RegistryFull)?;

        self.legacy_endpoints[endpoint_slot] = Some(LegacyEndpoint {
            owner,
            ..LegacyEndpoint::EMPTY
        });
        self.handle_spaces[handle_slot] = Some(HandleSpace {
            owner,
            ..HandleSpace::EMPTY
        });
        Ok(())
    }

    fn unregister(&mut self, owner: u64) -> Result<(), Error> {
        if self.shared_memory.iter().flatten().any(|object| {
            object
                .mappings
                .iter()
                .flatten()
                .any(|mapping| mapping.owner == owner)
        }) {
            return Err(Error::MappingBusy);
        }
        let endpoint_slot = self
            .legacy_endpoints
            .iter()
            .position(|endpoint| endpoint.is_some_and(|endpoint| endpoint.owner == owner))
            .ok_or(Error::NoEndpoint)?;
        let handle_slot = self
            .handle_spaces
            .iter()
            .position(|space| space.is_some_and(|space| space.owner == owner))
            .ok_or(Error::NoHandleSpace)?;

        // Stage 8.6: service registrations are owned by the publishing process.
        // Drop those escrow references before draining its ordinary handles so
        // process exit cannot leave a stale discoverable service behind.
        for index in 0..MAX_SERVICES {
            if self.services[index].is_some_and(|service| service.owner == owner) {
                if let Some(service) = self.services[index].take() {
                    self.release_object_reference(service.object);
                }
            }
        }

        let mut space = self.handle_spaces[handle_slot]
            .take()
            .ok_or(Error::NoHandleSpace)?;
        for object in self.objects.iter_mut().flatten() {
            remove_waiters_for_owner(&mut object.receive_waiters, owner);
            remove_waiters_for_owner(&mut object.send_waiters, owner);
        }
        for entry in space.entries.iter_mut() {
            if let Some(entry) = entry.take() {
                self.release_object_reference(entry.object);
            }
        }
        self.legacy_endpoints[endpoint_slot] = None;
        Ok(())
    }

    fn send(&mut self, sender: u64, receiver: u64, payload: &[u8]) -> Result<(), Error> {
        if payload.len() > MAX_MESSAGE_SIZE {
            return Err(Error::MessageTooLarge);
        }
        if self.legacy_endpoint(sender).is_none() {
            return Err(Error::NoEndpoint);
        }
        let endpoint = self
            .legacy_endpoint_mut(receiver)
            .ok_or(Error::NoEndpoint)?;
        let mut message = Message {
            sender,
            ..Message::EMPTY
        };
        message.length = payload.len();
        message.bytes[..payload.len()].copy_from_slice(payload);
        endpoint.enqueue(message)
    }

    fn receive(&mut self, receiver: u64) -> Result<Message, Error> {
        self.legacy_endpoint_mut(receiver)
            .ok_or(Error::NoEndpoint)?
            .dequeue()
    }

    fn peek(&self, receiver: u64) -> Result<Message, Error> {
        let endpoint = self.legacy_endpoint(receiver).ok_or(Error::NoEndpoint)?;
        endpoint
            .messages
            .first()
            .copied()
            .filter(|_| endpoint.length != 0)
            .ok_or(Error::QueueEmpty)
    }

    fn create_endpoint_object(&mut self, owner: u64) -> Result<Handle, Error> {
        if self.handle_space(owner).is_none() {
            return Err(Error::NoHandleSpace);
        }
        let object_slot = self
            .objects
            .iter()
            .position(Option::is_none)
            .ok_or(Error::ObjectTableFull)?;
        let object_id = EndpointObjectId(self.next_object_id);
        self.next_object_id = self.next_object_id.wrapping_add(1).max(1);

        self.objects[object_slot] = Some(EndpointObject {
            id: object_id,
            owner,
            references: 1,
            messages: [QueuedMessage::EMPTY; QUEUE_DEPTH],
            length: 0,
            receive_waiters: [None; MAX_WAITERS],
            send_waiters: [None; MAX_WAITERS],
            delegation_lineage: None,
        });
        let handle = match self.handle_space_mut(owner) {
            Some(space) => space.allocate(
                ObjectRef {
                    id: object_id,
                    kind: HandleObjectKind::Endpoint,
                },
                HandleRights::ENDPOINT_OWNER,
                None,
            ),
            None => Err(Error::NoHandleSpace),
        };
        match handle {
            Ok(handle) => Ok(handle),
            Err(error) => {
                self.objects[object_slot] = None;
                Err(error)
            }
        }
    }

    fn delegation_lineage_for(
        &mut self,
        owner: u64,
        entry: HandleEntry,
    ) -> Result<LineageId, Error> {
        if let Some(lineage) = entry.lineage {
            if wovenguard::lineage_authorizes(lineage, Capability::Ipc) {
                return Ok(lineage);
            }
            return Err(Error::AccessDenied);
        }
        let rights = CapabilitySet::only(Capability::Ipc);
        match entry.object.kind {
            HandleObjectKind::Endpoint => {
                let object = self.object_mut(entry.object).ok_or(Error::InvalidHandle)?;
                if object.owner != owner {
                    return Err(Error::AccessDenied);
                }
                if let Some(lineage) = object.delegation_lineage {
                    if wovenguard::lineage_authorizes(lineage, Capability::Ipc) {
                        return Ok(lineage);
                    }
                    object.delegation_lineage = None;
                }
                let lineage = wovenguard::issue_lineage_root(owner, rights)
                    .map_err(|_| Error::AccessDenied)?;
                object.delegation_lineage = Some(lineage);
                Ok(lineage)
            }
            HandleObjectKind::SharedMemory => {
                let object = self
                    .shared_object_mut(entry.object)
                    .ok_or(Error::InvalidHandle)?;
                if object.owner != owner {
                    return Err(Error::AccessDenied);
                }
                if let Some(lineage) = object.delegation_lineage {
                    if wovenguard::lineage_authorizes(lineage, Capability::Ipc) {
                        return Ok(lineage);
                    }
                    object.delegation_lineage = None;
                }
                let lineage = wovenguard::issue_lineage_root(owner, rights)
                    .map_err(|_| Error::AccessDenied)?;
                object.delegation_lineage = Some(lineage);
                Ok(lineage)
            }
        }
    }

    fn grant_handle(
        &mut self,
        owner: u64,
        handle: Handle,
        target: u64,
        rights: u8,
    ) -> Result<Handle, Error> {
        if rights == 0 {
            return Err(Error::AccessDenied);
        }
        let source = self
            .handle_space(owner)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        let allowed = match source.object.kind {
            HandleObjectKind::Endpoint => HandleRights::ENDPOINT_OWNER,
            HandleObjectKind::SharedMemory => HandleRights::SHARED_MEMORY_OWNER,
        };
        if rights & !allowed != 0
            || source.rights & HandleRights::TRANSFER == 0
            || rights & !source.rights != 0
        {
            return Err(Error::AccessDenied);
        }
        let lineage = self.delegation_lineage_for(owner, source)?;
        self.retain_object_reference(source.object)?;
        match self
            .handle_space_mut(target)
            .ok_or(Error::NoHandleSpace)
            .and_then(|space| {
                space.allocate_tagged(source.object, rights, Some(lineage), source.service_class)
            }) {
            Ok(target_handle) => Ok(target_handle),
            Err(error) => {
                self.release_object_reference(source.object);
                Err(error)
            }
        }
    }

    fn install_shared_memory(
        &mut self,
        owner: u64,
        frames: [u64; MAX_SHM_PAGES],
        page_count: usize,
    ) -> Result<Handle, Error> {
        if page_count == 0 || page_count > MAX_SHM_PAGES {
            return Err(Error::InvalidSize);
        }
        if self.handle_space(owner).is_none() {
            return Err(Error::NoHandleSpace);
        }
        let slot = self
            .shared_memory
            .iter()
            .position(Option::is_none)
            .ok_or(Error::ObjectTableFull)?;
        let id = EndpointObjectId(self.next_object_id);
        self.next_object_id = self.next_object_id.wrapping_add(1).max(1);
        let object_ref = ObjectRef {
            id,
            kind: HandleObjectKind::SharedMemory,
        };
        self.shared_memory[slot] = Some(SharedMemoryObject {
            id,
            owner,
            references: 1,
            page_count,
            frames,
            mappings: [None; MAX_SHM_MAPPINGS],
            delegation_lineage: None,
        });
        match self
            .handle_space_mut(owner)
            .ok_or(Error::NoHandleSpace)
            .and_then(|space| space.allocate(object_ref, HandleRights::SHARED_MEMORY_OWNER, None))
        {
            Ok(handle) => Ok(handle),
            Err(error) => {
                self.shared_memory[slot] = None;
                Err(error)
            }
        }
    }

    fn shared_memory_info(&self, owner: u64, handle: Handle) -> Result<SharedMemoryInfo, Error> {
        let entry = self
            .handle_space(owner)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        if entry.object.kind != HandleObjectKind::SharedMemory
            || entry.rights & HandleRights::INSPECT == 0
        {
            return Err(Error::AccessDenied);
        }
        let object = self
            .shared_object(entry.object)
            .ok_or(Error::InvalidHandle)?;
        Ok(SharedMemoryInfo {
            object_id: object.id,
            owner: object.owner,
            rights: entry.rights,
            page_count: object.page_count,
            mapping_count: object.mappings.iter().flatten().count(),
        })
    }

    fn prepare_shared_io(
        &mut self,
        owner: u64,
        handle: Handle,
        write: bool,
    ) -> Result<(ObjectRef, [u64; MAX_SHM_PAGES], usize), Error> {
        let entry = self
            .handle_space(owner)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        if entry.object.kind != HandleObjectKind::SharedMemory {
            return Err(Error::InvalidHandle);
        }
        let required = if write {
            HandleRights::SHM_WRITE
        } else {
            HandleRights::SHM_READ
        };
        if entry.rights & required == 0 {
            return Err(Error::AccessDenied);
        }
        let object = self
            .shared_object(entry.object)
            .ok_or(Error::InvalidHandle)?;
        let frames = object.frames;
        let page_count = object.page_count;
        self.retain_object_reference(entry.object)?;
        Ok((entry.object, frames, page_count))
    }

    fn prepare_shared_mapping(
        &mut self,
        owner: u64,
        handle: Handle,
        space: paging::AddressSpace,
        start: u64,
        writable: bool,
    ) -> Result<(ObjectRef, [u64; MAX_SHM_PAGES], usize), Error> {
        let entry = self
            .handle_space(owner)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        if entry.object.kind != HandleObjectKind::SharedMemory
            || entry.rights & HandleRights::SHM_MAP == 0
            || entry.rights & HandleRights::SHM_READ == 0
            || (writable && entry.rights & HandleRights::SHM_WRITE == 0)
        {
            return Err(Error::AccessDenied);
        }
        let object = self
            .shared_object(entry.object)
            .ok_or(Error::InvalidHandle)?;
        if object.mappings.iter().flatten().any(|mapping| {
            mapping.owner == owner && mapping.space == space && mapping.start == start
        }) {
            return Err(Error::AlreadyMapped);
        }
        if object.mappings.iter().all(Option::is_some) {
            return Err(Error::MappingTableFull);
        }
        let frames = object.frames;
        let page_count = object.page_count;
        self.retain_object_reference(entry.object)?;
        Ok((entry.object, frames, page_count))
    }

    fn finish_shared_mapping(
        &mut self,
        object_ref: ObjectRef,
        owner: u64,
        space: paging::AddressSpace,
        start: u64,
        _writable: bool,
    ) -> Result<(), Error> {
        let object = self
            .shared_object_mut(object_ref)
            .ok_or(Error::InvalidHandle)?;
        let slot = object
            .mappings
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Error::MappingTableFull)?;
        *slot = Some(SharedMapping {
            owner,
            space,
            start,
        });
        Ok(())
    }

    fn cancel_shared_mapping(&mut self, object_ref: ObjectRef) {
        self.release_object_reference(object_ref);
    }

    fn take_shared_mapping(
        &mut self,
        owner: u64,
        space: paging::AddressSpace,
        start: u64,
    ) -> Result<(ObjectRef, SharedMapping, usize), Error> {
        for index in 0..self.shared_memory.len() {
            let Some(object) = self.shared_memory[index].as_mut() else {
                continue;
            };
            let Some(mapping_slot) = object.mappings.iter().position(|slot| {
                slot.is_some_and(|mapping| {
                    mapping.owner == owner && mapping.space == space && mapping.start == start
                })
            }) else {
                continue;
            };
            let mapping = object.mappings[mapping_slot]
                .take()
                .ok_or(Error::NotMapped)?;
            let object_ref = ObjectRef {
                id: object.id,
                kind: HandleObjectKind::SharedMemory,
            };
            return Ok((object_ref, mapping, object.page_count));
        }
        Err(Error::NotMapped)
    }

    fn restore_shared_mapping(
        &mut self,
        object_ref: ObjectRef,
        mapping: SharedMapping,
    ) -> Result<(), Error> {
        let object = self
            .shared_object_mut(object_ref)
            .ok_or(Error::InvalidHandle)?;
        let slot = object
            .mappings
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Error::MappingTableFull)?;
        *slot = Some(mapping);
        Ok(())
    }

    fn send_handle(
        &mut self,
        sender: u64,
        handle: Handle,
        payload: &[u8],
    ) -> Result<Option<TaskId>, Error> {
        self.send_handle_with_optional_transfer(sender, handle, None, payload)
    }

    fn would_create_endpoint_transfer_cycle(
        &self,
        destination: ObjectRef,
        transfer: ObjectRef,
    ) -> bool {
        if transfer.kind != HandleObjectKind::Endpoint {
            return false;
        }
        if destination == transfer {
            return true;
        }

        let mut stack = [None; MAX_OBJECTS];
        let mut stack_len = 1usize;
        stack[0] = Some(transfer);
        let mut visited = [None; MAX_OBJECTS];
        let mut visited_len = 0usize;

        while stack_len != 0 {
            stack_len -= 1;
            let Some(current) = stack[stack_len].take() else {
                continue;
            };
            if current == destination {
                return true;
            }
            if visited[..visited_len].contains(&Some(current)) {
                continue;
            }
            if visited_len == MAX_OBJECTS {
                return true;
            }
            visited[visited_len] = Some(current);
            visited_len += 1;

            let Some(endpoint) = self.object(current) else {
                continue;
            };
            for queued in endpoint.messages.iter().take(endpoint.length) {
                let Some(next) = queued.transfer.map(|escrow| escrow.object) else {
                    continue;
                };
                if next.kind != HandleObjectKind::Endpoint
                    || visited[..visited_len].contains(&Some(next))
                {
                    continue;
                }
                if stack_len == MAX_OBJECTS {
                    return true;
                }
                stack[stack_len] = Some(next);
                stack_len += 1;
            }
        }
        false
    }

    fn send_handle_with_transfer(
        &mut self,
        sender: u64,
        endpoint_handle: Handle,
        transfer_handle: Handle,
        rights: u8,
        payload: &[u8],
    ) -> Result<Option<TaskId>, Error> {
        if rights == 0 {
            return Err(Error::AccessDenied);
        }
        let transfer = self
            .handle_space(sender)
            .ok_or(Error::NoHandleSpace)?
            .lookup(transfer_handle)?;
        let allowed = match transfer.object.kind {
            HandleObjectKind::Endpoint => HandleRights::ENDPOINT_OWNER,
            HandleObjectKind::SharedMemory => HandleRights::SHARED_MEMORY_OWNER,
        };
        if transfer.rights & HandleRights::TRANSFER == 0
            || rights & !allowed != 0
            || rights & !transfer.rights != 0
        {
            return Err(Error::AccessDenied);
        }
        let lineage = self.delegation_lineage_for(sender, transfer)?;

        let endpoint = self
            .handle_space(sender)
            .ok_or(Error::NoHandleSpace)?
            .lookup(endpoint_handle)?;
        if endpoint.rights & HandleRights::SEND == 0 {
            return Err(Error::AccessDenied);
        }
        if endpoint.object.kind != HandleObjectKind::Endpoint {
            return Err(Error::InvalidHandle);
        }
        if self.would_create_endpoint_transfer_cycle(endpoint.object, transfer.object) {
            // Queued capability escrows are owning references. Keep the bounded
            // endpoint graph acyclic so pure reference counting can reclaim it
            // deterministically without a tracing garbage collector.
            return Err(Error::AccessDenied);
        }

        self.retain_object_reference(transfer.object)?;
        let result = self.send_handle_with_optional_transfer(
            sender,
            endpoint_handle,
            Some(TransferEscrow {
                object: transfer.object,
                rights,
                lineage: Some(lineage),
                service_class: transfer.service_class,
            }),
            payload,
        );
        if result.is_err() {
            self.release_object_reference(transfer.object);
        }
        result
    }

    fn send_handle_with_optional_transfer(
        &mut self,
        sender: u64,
        handle: Handle,
        transfer: Option<TransferEscrow>,
        payload: &[u8],
    ) -> Result<Option<TaskId>, Error> {
        if payload.len() > MAX_MESSAGE_SIZE {
            return Err(Error::MessageTooLarge);
        }
        let entry = self
            .handle_space(sender)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        if entry.rights & HandleRights::SEND == 0 {
            return Err(Error::AccessDenied);
        }
        let object = self.object_mut(entry.object).ok_or(Error::InvalidHandle)?;
        let mut message = Message {
            sender,
            ..Message::EMPTY
        };
        message.length = payload.len();
        message.bytes[..payload.len()].copy_from_slice(payload);
        object.enqueue(QueuedMessage { message, transfer })?;
        Ok(take_waiter(&mut object.receive_waiters).map(|waiter| waiter.task))
    }

    fn receive_handle(
        &mut self,
        receiver: u64,
        handle: Handle,
    ) -> Result<(Message, Option<TaskId>), Error> {
        let endpoint_entry = self
            .handle_space(receiver)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        if endpoint_entry.rights & HandleRights::RECEIVE == 0 {
            return Err(Error::AccessDenied);
        }
        let queued = self
            .object(endpoint_entry.object)
            .ok_or(Error::InvalidHandle)?
            .head()?;

        let installed = if let Some(transfer) = queued.transfer {
            // Allocate before dequeue. If the receiver has no free handle slot,
            // the message and its escrow reference remain untouched in the queue.
            Some(
                self.handle_space_mut(receiver)
                    .ok_or(Error::NoHandleSpace)?
                    .allocate_tagged(
                        transfer.object,
                        transfer.rights,
                        transfer.lineage,
                        transfer.service_class,
                    )?,
            )
        } else {
            None
        };

        let object = self
            .object_mut(endpoint_entry.object)
            .ok_or(Error::InvalidHandle)?;
        let dequeued = object.dequeue()?;
        let wake = take_waiter(&mut object.send_waiters).map(|waiter| waiter.task);
        let mut message = dequeued.message;
        message.transferred_handle = installed;
        // The queue escrow's single object reference is consumed directly by
        // the newly installed receiver handle; no retain/release pair is needed.
        Ok((message, wake))
    }

    fn register_receive_waiter(
        &mut self,
        receiver: u64,
        handle: Handle,
        task: TaskId,
    ) -> Result<(), Error> {
        let entry = self
            .handle_space(receiver)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        if entry.rights & HandleRights::RECEIVE == 0 {
            return Err(Error::AccessDenied);
        }
        let object = self.object_mut(entry.object).ok_or(Error::InvalidHandle)?;
        if object.length != 0 {
            return Ok(());
        }
        push_waiter(
            &mut object.receive_waiters,
            Waiter {
                owner: receiver,
                handle,
                task,
            },
        )
    }

    fn register_send_waiter(
        &mut self,
        sender: u64,
        handle: Handle,
        task: TaskId,
    ) -> Result<(), Error> {
        let entry = self
            .handle_space(sender)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        if entry.rights & HandleRights::SEND == 0 {
            return Err(Error::AccessDenied);
        }
        let object = self.object_mut(entry.object).ok_or(Error::InvalidHandle)?;
        if object.length != QUEUE_DEPTH {
            return Ok(());
        }
        push_waiter(
            &mut object.send_waiters,
            Waiter {
                owner: sender,
                handle,
                task,
            },
        )
    }

    fn waiter_counts(&self, owner: u64, handle: Handle) -> Result<(usize, usize), Error> {
        let entry = self
            .handle_space(owner)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        if entry.rights & HandleRights::INSPECT == 0 {
            return Err(Error::AccessDenied);
        }
        let object = self.object(entry.object).ok_or(Error::InvalidHandle)?;
        Ok((
            object.receive_waiters.iter().flatten().count(),
            object.send_waiters.iter().flatten().count(),
        ))
    }

    fn publish_service(
        &mut self,
        owner: u64,
        name: &[u8],
        handle: Handle,
        client_rights: u8,
    ) -> Result<(), Error> {
        let service_name = ServiceName::parse(name)?;
        if client_rights == 0 || client_rights & !HandleRights::SERVICE_CLIENT != 0 {
            return Err(Error::AccessDenied);
        }
        if self
            .services
            .iter()
            .flatten()
            .any(|service| service.name.matches(name))
        {
            return Err(Error::ServiceExists);
        }
        let source = self
            .handle_space(owner)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        if source.object.kind != HandleObjectKind::Endpoint
            || source.rights & HandleRights::PUBLISH_SERVICE == 0
            || client_rights & !source.rights != 0
        {
            return Err(Error::AccessDenied);
        }
        let lineage = self.delegation_lineage_for(owner, source)?;
        let slot = self
            .services
            .iter()
            .position(Option::is_none)
            .ok_or(Error::ServiceTableFull)?;
        self.retain_object_reference(source.object)?;
        self.services[slot] = Some(ServiceEntry {
            name: service_name,
            owner,
            object: source.object,
            client_rights,
            lineage: Some(lineage),
            class: wovenguard::classify_service(name),
        });
        Ok(())
    }

    fn unpublish_service(&mut self, owner: u64, name: &[u8]) -> Result<(), Error> {
        ServiceName::parse(name)?;
        let slot = self
            .services
            .iter()
            .position(|entry| entry.is_some_and(|service| service.name.matches(name)))
            .ok_or(Error::ServiceNotFound)?;
        let service = self.services[slot].ok_or(Error::ServiceNotFound)?;
        if service.owner != owner {
            return Err(Error::AccessDenied);
        }
        self.services[slot] = None;
        self.release_object_reference(service.object);
        Ok(())
    }

    fn discover_service(&mut self, client: u64, name: &[u8]) -> Result<Handle, Error> {
        ServiceName::parse(name)?;
        if self.handle_space(client).is_none() {
            return Err(Error::NoHandleSpace);
        }
        let service = self
            .services
            .iter()
            .flatten()
            .find(|service| service.name.matches(name))
            .copied()
            .ok_or(Error::ServiceNotFound)?;
        if service
            .lineage
            .is_some_and(|lineage| !wovenguard::lineage_authorizes(lineage, Capability::Ipc))
        {
            return Err(Error::AccessDenied);
        }
        self.retain_object_reference(service.object)?;
        match self
            .handle_space_mut(client)
            .ok_or(Error::NoHandleSpace)
            .and_then(|space| {
                space.allocate_tagged(
                    service.object,
                    service.client_rights,
                    service.lineage,
                    Some(service.class),
                )
            }) {
            Ok(handle) => Ok(handle),
            Err(error) => {
                self.release_object_reference(service.object);
                Err(error)
            }
        }
    }

    fn service_info(&self, name: &[u8]) -> Result<ServiceInfo, Error> {
        ServiceName::parse(name)?;
        let service = self
            .services
            .iter()
            .flatten()
            .find(|service| service.name.matches(name))
            .ok_or(Error::ServiceNotFound)?;
        Ok(ServiceInfo {
            object_id: service.object.id,
            owner: service.owner,
            client_rights: service.client_rights,
            name_length: service.name.length as usize,
        })
    }

    fn service_count(&self) -> usize {
        self.services.iter().flatten().count()
    }

    fn revoke_object_delegations(&mut self, owner: u64, handle: Handle) -> Result<usize, Error> {
        let entry = self
            .handle_space(owner)
            .ok_or(Error::NoHandleSpace)?
            .lookup_raw(handle)?;
        let (object_owner, lineage) = match entry.object.kind {
            HandleObjectKind::Endpoint => {
                let object = self.object(entry.object).ok_or(Error::InvalidHandle)?;
                (object.owner, object.delegation_lineage)
            }
            HandleObjectKind::SharedMemory => {
                let object = self
                    .shared_object(entry.object)
                    .ok_or(Error::InvalidHandle)?;
                (object.owner, object.delegation_lineage)
            }
        };
        if object_owner != owner {
            return Err(Error::AccessDenied);
        }
        let lineage = lineage.ok_or(Error::AccessDenied)?;
        wovenguard::revoke_lineage_subtree(owner, lineage).map_err(|_| Error::AccessDenied)
    }

    fn close_handle(
        &mut self,
        owner: u64,
        handle: Handle,
    ) -> Result<[Option<TaskId>; MAX_WAITERS * 2], Error> {
        let entry = self
            .handle_space_mut(owner)
            .ok_or(Error::NoHandleSpace)?
            .remove(handle)?;

        let mut wake = [None; MAX_WAITERS * 2];
        if let Some(object) = self.object_mut(entry.object) {
            let receive = remove_waiters_for_handle(&mut object.receive_waiters, owner, handle);
            let send = remove_waiters_for_handle(&mut object.send_waiters, owner, handle);
            for task in receive.into_iter().chain(send).flatten() {
                if let Some(slot) = wake.iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(task);
                }
            }
        }
        self.release_object_reference(entry.object);
        Ok(wake)
    }

    fn handle_info(&self, owner: u64, handle: Handle) -> Result<HandleInfo, Error> {
        let entry = self
            .handle_space(owner)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?;
        let object_owner = match entry.object.kind {
            HandleObjectKind::Endpoint => {
                self.object(entry.object).ok_or(Error::InvalidHandle)?.owner
            }
            HandleObjectKind::SharedMemory => {
                self.shared_object(entry.object)
                    .ok_or(Error::InvalidHandle)?
                    .owner
            }
        };
        Ok(HandleInfo {
            object_id: entry.object.id,
            owner: object_owner,
            rights: entry.rights,
            kind: entry.object.kind,
        })
    }

    fn retain_object_reference(&mut self, object: ObjectRef) -> Result<(), Error> {
        match object.kind {
            HandleObjectKind::Endpoint => {
                let endpoint = self.object_mut(object).ok_or(Error::InvalidHandle)?;
                endpoint.references = endpoint
                    .references
                    .checked_add(1)
                    .ok_or(Error::ObjectTableFull)?;
            }
            HandleObjectKind::SharedMemory => {
                let shared = self.shared_object_mut(object).ok_or(Error::InvalidHandle)?;
                shared.references = shared
                    .references
                    .checked_add(1)
                    .ok_or(Error::ObjectTableFull)?;
            }
        }
        Ok(())
    }

    fn release_object_reference(&mut self, object: ObjectRef) {
        match object.kind {
            HandleObjectKind::Endpoint => {
                let Some(slot) = self
                    .objects
                    .iter()
                    .position(|entry| entry.is_some_and(|current| current.id == object.id))
                else {
                    return;
                };
                let Some(current) = self.objects[slot].as_mut() else {
                    return;
                };
                current.references = current.references.saturating_sub(1);
                if current.references == 0 {
                    let messages = current.messages;
                    let length = current.length;
                    let owner = current.owner;
                    let delegation_lineage = current.delegation_lineage;
                    self.objects[slot] = None;
                    if let Some(lineage) = delegation_lineage {
                        let _ = wovenguard::release_lineage(owner, lineage);
                    }
                    for queued in messages.into_iter().take(length) {
                        if let Some(transfer) = queued.transfer {
                            self.release_object_reference(transfer.object);
                        }
                    }
                }
            }
            HandleObjectKind::SharedMemory => {
                let Some(slot) = self
                    .shared_memory
                    .iter()
                    .position(|entry| entry.is_some_and(|current| current.id == object.id))
                else {
                    return;
                };
                let Some(current) = self.shared_memory[slot].as_mut() else {
                    return;
                };
                current.references = current.references.saturating_sub(1);
                if current.references == 0 {
                    let frames = current.frames;
                    let page_count = current.page_count;
                    let owner = current.owner;
                    let delegation_lineage = current.delegation_lineage;
                    self.shared_memory[slot] = None;
                    if let Some(lineage) = delegation_lineage {
                        let _ = wovenguard::release_lineage(owner, lineage);
                    }
                    for frame in frames.into_iter().take(page_count) {
                        let _ = paging::release_shared_frame(frame);
                    }
                }
            }
        }
    }

    fn legacy_endpoint(&self, owner: u64) -> Option<&LegacyEndpoint> {
        self.legacy_endpoints
            .iter()
            .flatten()
            .find(|endpoint| endpoint.owner == owner)
    }

    fn legacy_endpoint_mut(&mut self, owner: u64) -> Option<&mut LegacyEndpoint> {
        self.legacy_endpoints
            .iter_mut()
            .flatten()
            .find(|endpoint| endpoint.owner == owner)
    }

    fn handle_space(&self, owner: u64) -> Option<&HandleSpace> {
        self.handle_spaces
            .iter()
            .flatten()
            .find(|space| space.owner == owner)
    }

    fn handle_space_mut(&mut self, owner: u64) -> Option<&mut HandleSpace> {
        self.handle_spaces
            .iter_mut()
            .flatten()
            .find(|space| space.owner == owner)
    }

    fn object(&self, object: ObjectRef) -> Option<&EndpointObject> {
        if object.kind != HandleObjectKind::Endpoint {
            return None;
        }
        self.objects
            .iter()
            .flatten()
            .find(|current| current.id == object.id)
    }

    fn object_mut(&mut self, object: ObjectRef) -> Option<&mut EndpointObject> {
        if object.kind != HandleObjectKind::Endpoint {
            return None;
        }
        self.objects
            .iter_mut()
            .flatten()
            .find(|current| current.id == object.id)
    }

    fn shared_object(&self, object: ObjectRef) -> Option<&SharedMemoryObject> {
        if object.kind != HandleObjectKind::SharedMemory {
            return None;
        }
        self.shared_memory
            .iter()
            .flatten()
            .find(|current| current.id == object.id)
    }

    fn shared_object_mut(&mut self, object: ObjectRef) -> Option<&mut SharedMemoryObject> {
        if object.kind != HandleObjectKind::SharedMemory {
            return None;
        }
        self.shared_memory
            .iter_mut()
            .flatten()
            .find(|current| current.id == object.id)
    }

    fn handle_service_class(
        &self,
        owner: u64,
        handle: Handle,
    ) -> Result<Option<ServiceClass>, Error> {
        Ok(self
            .handle_space(owner)
            .ok_or(Error::NoHandleSpace)?
            .lookup(handle)?
            .service_class)
    }

    fn object_count(&self) -> usize {
        self.objects.iter().flatten().count() + self.shared_memory.iter().flatten().count()
    }

    fn shared_memory_count(&self) -> usize {
        self.shared_memory.iter().flatten().count()
    }
}

/// Global IPC namespace lock. Rank 10 keeps object/handle transactions
/// interrupt-safe and places IPC alongside the scheduler/teardown registries;
/// paging and frame release may safely nest at their higher ranks.
static STATE: Mutex<State> = Mutex::with_rank(State::new(), 10);

pub fn register(owner: u64) -> Result<(), Error> {
    STATE.lock().register(owner)
}

pub fn unregister(owner: u64) -> Result<(), Error> {
    STATE.lock().unregister(owner)
}

pub fn send(sender: u64, receiver: u64, payload: &[u8]) -> Result<(), Error> {
    STATE.lock().send(sender, receiver, payload)
}

pub fn receive(receiver: u64) -> Result<Message, Error> {
    STATE.lock().receive(receiver)
}

pub fn peek(receiver: u64) -> Result<Message, Error> {
    STATE.lock().peek(receiver)
}

/// Stage 8.1: create a kernel-owned IPC endpoint object and install an opaque
/// handle in the owning process's IPC handle space.
pub fn create_endpoint_object(owner: u64) -> Result<Handle, Error> {
    STATE.lock().create_endpoint_object(owner)
}

/// Stage 8.2: install a restricted handle to the same endpoint object in a
/// second process. This is kernel-mediated plumbing for now; general userspace
/// handle transfer remains a later stage. The requested rights must be a subset
/// of the source handle's rights and the source must carry TRANSFER authority.
pub fn grant_handle(owner: u64, handle: Handle, target: u64, rights: u8) -> Result<Handle, Error> {
    authorize_service_handle_use(owner, handle)?;
    let service_class = STATE.lock().handle_service_class(owner, handle)?;
    if let (Some(class), Some(profile)) = (service_class, task::sandbox_profile_for_process(target))
    {
        let decision = wovenguard::authorize_service_discover(profile, class);
        crate::audit::record_detail(
            target,
            crate::audit::Action::SandboxServiceDiscover,
            handle.as_u32() as u64,
            ((profile.id() as u64) << 32) | class as u64,
            decision.allowed,
        );
        if !decision.allowed {
            return Err(Error::AccessDenied);
        }
    }
    STATE.lock().grant_handle(owner, handle, target, rights)
}

/// Stage 8.4: create a bounded shared-memory object backed by zeroed 4 KiB
/// frames and install its owner handle in the creating process. No userspace
/// syscall is exposed yet; this is the kernel object/mapping foundation.
pub fn create_shared_memory(owner: u64, page_count: usize) -> Result<Handle, Error> {
    if page_count == 0 || page_count > MAX_SHM_PAGES {
        return Err(Error::InvalidSize);
    }
    let mut frames = [0_u64; MAX_SHM_PAGES];
    for index in 0..page_count {
        let Some(frame) = paging::allocate_shared_frame() else {
            for physical in frames.into_iter().take(index) {
                let _ = paging::release_shared_frame(physical);
            }
            return Err(Error::MappingFailed);
        };
        frames[index] = frame;
    }
    match STATE
        .lock()
        .install_shared_memory(owner, frames, page_count)
    {
        Ok(handle) => Ok(handle),
        Err(error) => {
            for physical in frames.into_iter().take(page_count) {
                let _ = paging::release_shared_frame(physical);
            }
            Err(error)
        }
    }
}

/// Read shared-memory bytes through the kernel's direct physical mapping.
/// This is primarily an object-layer primitive and rights proof for Stage 8.4.
pub fn read_shared_memory(
    owner: u64,
    handle: Handle,
    offset: usize,
    output: &mut [u8],
) -> Result<(), Error> {
    let (object_ref, frames, page_count) = {
        let mut state = STATE.lock();
        state.prepare_shared_io(owner, handle, false)?
    };
    let total = page_count.checked_mul(4096).ok_or(Error::InvalidSize);
    let result = match total {
        Ok(total) if offset <= total && output.len() <= total.saturating_sub(offset) => {
            let mut copied = 0usize;
            while copied < output.len() {
                let absolute = offset + copied;
                let page = absolute / 4096;
                let in_page = absolute % 4096;
                let count = core::cmp::min(4096 - in_page, output.len() - copied);
                paging::read_file_frame(frames[page], in_page, &mut output[copied..copied + count]);
                copied += count;
            }
            Ok(())
        }
        _ => Err(Error::InvalidSize),
    };
    STATE.lock().release_object_reference(object_ref);
    result
}

/// Write shared-memory bytes through the kernel's direct physical mapping.
pub fn write_shared_memory(
    owner: u64,
    handle: Handle,
    offset: usize,
    bytes: &[u8],
) -> Result<(), Error> {
    let (object_ref, frames, page_count) = {
        let mut state = STATE.lock();
        state.prepare_shared_io(owner, handle, true)?
    };
    let total = page_count.checked_mul(4096).ok_or(Error::InvalidSize);
    let result = match total {
        Ok(total) if offset <= total && bytes.len() <= total.saturating_sub(offset) => {
            let mut copied = 0usize;
            while copied < bytes.len() {
                let absolute = offset + copied;
                let page = absolute / 4096;
                let in_page = absolute % 4096;
                let count = core::cmp::min(4096 - in_page, bytes.len() - copied);
                paging::write_file_frame(frames[page], in_page, &bytes[copied..copied + count]);
                copied += count;
            }
            Ok(())
        }
        _ => Err(Error::InvalidSize),
    };
    STATE.lock().release_object_reference(object_ref);
    result
}

/// Map every page of a shared-memory object into `space` at `start`. The IPC
/// state lock is deliberately dropped before touching page tables. A temporary
/// object reference pins the backing frames across that unlocked interval; on
/// success the reference becomes the mapping's lifetime reference.
pub fn map_shared_memory(
    owner: u64,
    handle: Handle,
    space: paging::AddressSpace,
    start: u64,
    writable: bool,
) -> Result<usize, Error> {
    if start & 4095 != 0 || start.checked_add((MAX_SHM_PAGES * 4096) as u64).is_none() {
        return Err(Error::InvalidSize);
    }
    let (object_ref, frames, page_count) = {
        let mut state = STATE.lock();
        state.prepare_shared_mapping(owner, handle, space, start, writable)?
    };

    let mut mapped = 0usize;
    for (index, physical) in frames.into_iter().take(page_count).enumerate() {
        let address = start
            .checked_add((index * 4096) as u64)
            .ok_or(Error::InvalidSize)?;
        if !paging::map_shared_frame(space, address, physical, writable) {
            if mapped != 0 {
                let _ = paging::unmap_user_range_in(space, start, mapped * 4096);
            }
            STATE.lock().cancel_shared_mapping(object_ref);
            return Err(Error::MappingFailed);
        }
        mapped += 1;
    }

    let finish = STATE
        .lock()
        .finish_shared_mapping(object_ref, owner, space, start, writable);
    if let Err(error) = finish {
        let _ = paging::unmap_user_range_in(space, start, mapped * 4096);
        STATE.lock().cancel_shared_mapping(object_ref);
        return Err(error);
    }
    Ok(page_count * 4096)
}

/// Remove one shared-memory mapping owned by `owner`. The mapping registry is
/// detached before the paging operation and restored if unmap fails, keeping
/// the object/mapping lifetime transactional.
pub fn unmap_shared_memory(
    owner: u64,
    space: paging::AddressSpace,
    start: u64,
) -> Result<(), Error> {
    let (object_ref, mapping, page_count) =
        STATE.lock().take_shared_mapping(owner, space, start)?;
    if paging::unmap_user_range_in(space, start, page_count * 4096).is_err() {
        let _ = STATE.lock().restore_shared_mapping(object_ref, mapping);
        return Err(Error::MappingFailed);
    }
    STATE.lock().cancel_shared_mapping(object_ref);
    Ok(())
}

pub fn shared_memory_info(owner: u64, handle: Handle) -> Result<SharedMemoryInfo, Error> {
    STATE.lock().shared_memory_info(owner, handle)
}

pub fn shared_memory_count() -> usize {
    STATE.lock().shared_memory_count()
}

/// Stage 8.6: publish an endpoint under a bounded global service name. The
/// registry retains the endpoint independently of the publisher's source handle.
fn service_name_hash(name: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in name {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn audit_service_policy(
    owner: u64,
    action: crate::audit::Action,
    name: &[u8],
    class: ServiceClass,
    profile: wovenguard::SandboxProfile,
    allowed: bool,
) {
    let detail = ((profile.id() as u64) << 32) | class as u64;
    crate::audit::record_detail(owner, action, service_name_hash(name), detail, allowed);
}

fn authorize_service_publish_for(owner: u64, name: &[u8]) -> Result<(), Error> {
    let class = wovenguard::classify_service(name);
    if let Some(profile) = task::sandbox_profile_for_process(owner) {
        let decision = wovenguard::authorize_service_publish(profile, class);
        audit_service_policy(
            owner,
            crate::audit::Action::SandboxServicePublish,
            name,
            class,
            profile,
            decision.allowed,
        );
        if !decision.allowed {
            return Err(Error::AccessDenied);
        }
    }
    Ok(())
}

fn authorize_service_discover_for(owner: u64, name: &[u8]) -> Result<(), Error> {
    let class = wovenguard::classify_service(name);
    if let Some(profile) = task::sandbox_profile_for_process(owner) {
        let decision = wovenguard::authorize_service_discover(profile, class);
        audit_service_policy(
            owner,
            crate::audit::Action::SandboxServiceDiscover,
            name,
            class,
            profile,
            decision.allowed,
        );
        if !decision.allowed {
            return Err(Error::AccessDenied);
        }
    }
    Ok(())
}

fn authorize_service_handle_use(owner: u64, handle: Handle) -> Result<(), Error> {
    let class = STATE.lock().handle_service_class(owner, handle)?;
    let Some(class) = class else {
        return Ok(());
    };
    if let Some(profile) = task::sandbox_profile_for_process(owner) {
        let decision = wovenguard::authorize_service_discover(profile, class);
        crate::audit::record_detail(
            owner,
            crate::audit::Action::SandboxServiceUse,
            handle.as_u32() as u64,
            ((profile.id() as u64) << 32) | class as u64,
            decision.allowed,
        );
        if !decision.allowed {
            return Err(Error::AccessDenied);
        }
    }
    Ok(())
}

pub fn publish_service(
    owner: u64,
    name: &[u8],
    handle: Handle,
    client_rights: u8,
) -> Result<(), Error> {
    let _ = ServiceName::parse(name)?;
    authorize_service_publish_for(owner, name)?;
    STATE
        .lock()
        .publish_service(owner, name, handle, client_rights)
}

/// Remove a service registration. Only the process that published the name may
/// remove it; process teardown also removes all services owned by that process.
pub fn unpublish_service(owner: u64, name: &[u8]) -> Result<(), Error> {
    STATE.lock().unpublish_service(owner, name)
}

/// Resolve a service name to a fresh process-local capability carrying exactly
/// the client rights selected by the service publisher.
pub fn discover_service(client: u64, name: &[u8]) -> Result<Handle, Error> {
    let _ = ServiceName::parse(name)?;
    authorize_service_discover_for(client, name)?;
    STATE.lock().discover_service(client, name)
}

pub fn service_info(name: &[u8]) -> Result<ServiceInfo, Error> {
    STATE.lock().service_info(name)
}

pub fn service_count() -> usize {
    STATE.lock().service_count()
}

/// Stage 9.2D: revoke every delegated handle/service/escrow authority derived
/// from this IPC object while leaving the creator's intrinsic owner handle intact.
pub fn revoke_object_delegations(owner: u64, handle: Handle) -> Result<usize, Error> {
    STATE.lock().revoke_object_delegations(owner, handle)
}

/// Stage 8.2 nonblocking handle-addressed send. Queue saturation returns
/// QueueFull; it never sleeps or drops a message.
pub fn send_handle(sender: u64, handle: Handle, payload: &[u8]) -> Result<(), Error> {
    authorize_service_handle_use(sender, handle)?;
    let wake = STATE.lock().send_handle(sender, handle, payload)?;
    if let Some(task_id) = wake {
        let _ = task::signal_event(task_id);
    }
    Ok(())
}

/// Stage 8.5: enqueue a message plus one reduced-rights capability. The source
/// handle must carry TRANSFER authority. The queue stores object identity and
/// rights, never the sender's process-local numeric handle.
pub fn send_handle_with_transfer(
    sender: u64,
    endpoint_handle: Handle,
    transfer_handle: Handle,
    rights: u8,
    payload: &[u8],
) -> Result<(), Error> {
    authorize_service_handle_use(sender, endpoint_handle)?;
    authorize_service_handle_use(sender, transfer_handle)?;
    let wake = STATE.lock().send_handle_with_transfer(
        sender,
        endpoint_handle,
        transfer_handle,
        rights,
        payload,
    )?;
    if let Some(task_id) = wake {
        let _ = task::signal_event(task_id);
    }
    Ok(())
}

/// Stage 8.2 nonblocking handle-addressed receive. An empty queue returns
/// QueueEmpty. Stage 8.3 adds `receive_handle_blocking` without changing this
/// already-validated ABI.
pub fn receive_handle(receiver: u64, handle: Handle) -> Result<Message, Error> {
    let (message, wake) = STATE.lock().receive_handle(receiver, handle)?;
    if let Some(task_id) = wake {
        let _ = task::signal_event(task_id);
    }
    Ok(message)
}

/// Stage 8.3 blocking handle send. Full queues register the current task as a
/// writable waiter, then use the scheduler's latched event primitive. A receive
/// can therefore signal either before or after the sender publishes Blocked
/// without losing the wakeup.
pub fn send_handle_blocking(sender: u64, handle: Handle, payload: &[u8]) -> Result<(), Error> {
    authorize_service_handle_use(sender, handle)?;
    loop {
        let task_id = task::current_task_id();
        {
            let mut state = STATE.lock();
            match state.send_handle(sender, handle, payload) {
                Ok(wake) => {
                    drop(state);
                    if let Some(task_id) = wake {
                        let _ = task::signal_event(task_id);
                    }
                    return Ok(());
                }
                Err(Error::QueueFull) => {
                    state.register_send_waiter(sender, handle, task_id)?;
                }
                Err(error) => return Err(error),
            }
        }
        task::wait_for_event();
    }
}

/// Stage 8.3 blocking handle receive. Empty queues register the current task as
/// a readable waiter before dropping the IPC lock. `signal_event` latches an
/// early signal in the TCB, closing the check/register/block race without
/// introducing an IPC->scheduler nested-lock dependency.
pub fn receive_handle_blocking(receiver: u64, handle: Handle) -> Result<Message, Error> {
    loop {
        let task_id = task::current_task_id();
        {
            let mut state = STATE.lock();
            match state.receive_handle(receiver, handle) {
                Ok((message, wake)) => {
                    drop(state);
                    if let Some(task_id) = wake {
                        let _ = task::signal_event(task_id);
                    }
                    return Ok(message);
                }
                Err(Error::QueueEmpty) => {
                    state.register_receive_waiter(receiver, handle, task_id)?;
                }
                Err(error) => return Err(error),
            }
        }
        task::wait_for_event();
    }
}

/// Introspection used by the Stage 8.3 runtime proof. Requires INSPECT rights
/// and is intentionally read-only.
pub fn waiter_counts(owner: u64, handle: Handle) -> Result<(usize, usize), Error> {
    STATE.lock().waiter_counts(owner, handle)
}

/// Stage 8.1: close only a handle belonging to `owner`. Handles are deliberately
/// process-local; the same numeric value in another process is not authority.
/// Stage 8.3 additionally wakes tasks waiting through this exact handle so they
/// can revalidate and observe `InvalidHandle` rather than sleeping forever.
pub fn close_handle(owner: u64, handle: Handle) -> Result<(), Error> {
    let wake = STATE.lock().close_handle(owner, handle)?;
    for task_id in wake.into_iter().flatten() {
        let _ = task::signal_event(task_id);
    }
    Ok(())
}

pub fn handle_info(owner: u64, handle: Handle) -> Result<HandleInfo, Error> {
    STATE.lock().handle_info(owner, handle)
}

pub fn endpoint_count() -> usize {
    STATE.lock().legacy_endpoints.iter().flatten().count()
}

pub fn object_count() -> usize {
    STATE.lock().object_count()
}

pub fn handle_count(owner: u64) -> usize {
    STATE
        .lock()
        .handle_space(owner)
        .map_or(0, HandleSpace::count)
}

/// Original PID-addressed queue regression. Kept separate so Stage 8.1 cannot
/// accidentally claim success by replacing the legacy ABI instead of preserving it.
///
/// This deliberately exercises the real global state instead of constructing a
/// full `State` on the bootstrap kernel stack. `State` contains the bounded IPC
/// object tables and, from Stage 8.4 onward, the shared-memory mapping registry;
/// placing a complete copy on the 1 MiB bootstrap stack is both unnecessary and
/// unsafe as those bounded tables grow.
pub fn self_test() -> bool {
    const SENDER: u64 = 10;
    const RECEIVER: u64 = 20;

    let mut state = STATE.lock();
    if state.legacy_endpoints.iter().flatten().next().is_some()
        || state.handle_spaces.iter().flatten().next().is_some()
        || state.object_count() != 0
        || state.shared_memory.iter().flatten().next().is_some()
    {
        return false;
    }

    let payload = b"wovenhat-ipc";
    let passed = (|| {
        state.register(SENDER).ok()?;
        state.register(RECEIVER).ok()?;
        if state.register(SENDER) != Err(Error::EndpointExists) {
            return None;
        }
        state.send(SENDER, RECEIVER, payload).ok()?;
        let message = state.receive(RECEIVER).ok()?;
        if message.sender != SENDER
            || message.payload() != payload
            || state.receive(RECEIVER) != Err(Error::QueueEmpty)
            || state.send(99, RECEIVER, payload) != Err(Error::NoEndpoint)
            || state.unregister(SENDER).is_err()
            || state.unregister(SENDER) != Err(Error::NoEndpoint)
        {
            return None;
        }
        Some(())
    })()
    .is_some();

    // Always restore the production registry to its pristine pre-test state,
    // including early-failure paths. SENDER may already have been removed by
    // the teardown assertion above, so both cleanup calls intentionally ignore
    // NoEndpoint.
    let _ = state.unregister(SENDER);
    let _ = state.unregister(RECEIVER);

    passed
        && state.legacy_endpoints.iter().flatten().next().is_none()
        && state.handle_spaces.iter().flatten().next().is_none()
        && state.object_count() == 0
        && state.shared_memory.iter().flatten().next().is_none()
}

/// Stage 8.1 regression for object identity, process-local authority, stale
/// handle rejection, bounded tables, and deterministic teardown.
///
/// Like `self_test`, this uses the real global registry to avoid a very large
/// stack-local `State` copy. The synthetic namespaces are cleaned on every
/// exit path before the scheduler starts creating normal process namespaces.
pub fn handle_object_self_test() -> bool {
    const OWNER: u64 = 100;
    const PEER: u64 = 200;

    let mut state = STATE.lock();
    if state.legacy_endpoints.iter().flatten().next().is_some()
        || state.handle_spaces.iter().flatten().next().is_some()
        || state.object_count() != 0
        || state.shared_memory.iter().flatten().next().is_some()
    {
        return false;
    }

    let passed = (|| {
        state.register(OWNER).ok()?;
        state.register(PEER).ok()?;

        let first = state.create_endpoint_object(OWNER).ok()?;
        let first_info = state.handle_info(OWNER, first).ok()?;
        if first_info.owner != OWNER
            || first_info.rights != HandleRights::ENDPOINT_OWNER
            || state.handle_info(PEER, first) != Err(Error::InvalidHandle)
            || state.object_count() != 1
            || state.handle_space(OWNER).map_or(0, HandleSpace::count) != 1
        {
            return None;
        }

        if state.close_handle(OWNER, first).is_err()
            || state.handle_info(OWNER, first) != Err(Error::InvalidHandle)
            || state.object_count() != 0
        {
            return None;
        }

        // Reusing the same handle-table slot must change the generation so the
        // stale handle cannot silently alias the replacement object.
        let second = state.create_endpoint_object(OWNER).ok()?;
        if second == first || state.handle_info(OWNER, first) != Err(Error::InvalidHandle) {
            return None;
        }

        // Process teardown closes every remaining handle and releases objects.
        if state.unregister(OWNER).is_err()
            || state.object_count() != 0
            || state.handle_space(OWNER).is_some()
            || state.legacy_endpoint(OWNER).is_some()
        {
            return None;
        }

        if state.unregister(PEER).is_err() {
            return None;
        }
        Some(())
    })()
    .is_some();

    // Failure-safe cleanup: if the probe stopped before either explicit
    // teardown above, unregister releases any endpoint handles it created.
    let _ = state.unregister(OWNER);
    let _ = state.unregister(PEER);

    passed
        && state.legacy_endpoints.iter().flatten().next().is_none()
        && state.handle_spaces.iter().flatten().next().is_none()
        && state.object_count() == 0
        && state.shared_memory.iter().flatten().next().is_none()
}

/// Runs against the real global IPC state after SMP is online. The probe uses a
/// reserved synthetic PID, cleans up synchronously, and leaves no persistent
/// endpoint/object/handle behind. This proves the production lock/state path,
/// not only the local pure-State self-test.
pub fn stage8_1_runtime_probe() -> bool {
    const PROBE_OWNER: u64 = u64::MAX - 8;

    // Exercise the production public API rather than reaching into State
    // directly. Besides proving the same path future syscall code will use,
    // this keeps the Stage 8.1 API surface live under the project's strict
    // `-D warnings` policy without suppressing dead-code diagnostics.
    if register(PROBE_OWNER).is_err() {
        return false;
    }

    let baseline_objects = object_count();
    let result = (|| {
        let first = create_endpoint_object(PROBE_OWNER).ok()?;
        let first_info = handle_info(PROBE_OWNER, first).ok()?;

        if first.as_u32() == 0
            || first_info.object_id.as_u64() == 0
            || first_info.owner != PROBE_OWNER
            || first_info.rights != HandleRights::ENDPOINT_OWNER
            || handle_count(PROBE_OWNER) != 1
            || object_count() != baseline_objects + 1
        {
            return None;
        }

        close_handle(PROBE_OWNER, first).ok()?;
        if handle_info(PROBE_OWNER, first) != Err(Error::InvalidHandle)
            || handle_count(PROBE_OWNER) != 0
            || object_count() != baseline_objects
        {
            return None;
        }

        // Reusing a slot must produce a new generation-tagged handle. Leave
        // the second object open so unregister() also proves process teardown.
        let second = create_endpoint_object(PROBE_OWNER).ok()?;
        if second == first
            || second.as_u32() == first.as_u32()
            || handle_count(PROBE_OWNER) != 1
            || object_count() != baseline_objects + 1
        {
            return None;
        }

        Some(())
    })()
    .is_some();

    let cleaned = unregister(PROBE_OWNER).is_ok()
        && handle_count(PROBE_OWNER) == 0
        && object_count() == baseline_objects;

    result && cleaned
}

/// Stage 8.2 production-state probe for bounded FIFO message passing and rights.
/// It uses two synthetic process namespaces and cleans every object/handle before
/// returning so the normal boot state is unchanged.
pub fn stage8_2_runtime_probe() -> bool {
    const SERVER: u64 = u64::MAX - 16;
    const CLIENT: u64 = u64::MAX - 17;

    if register(SERVER).is_err() || register(CLIENT).is_err() {
        let _ = unregister(SERVER);
        let _ = unregister(CLIENT);
        return false;
    }

    let baseline_objects = object_count();
    let result = (|| {
        let owner = create_endpoint_object(SERVER).ok()?;
        let sender = grant_handle(SERVER, owner, CLIENT, HandleRights::SEND).ok()?;
        let receiver = grant_handle(SERVER, owner, SERVER, HandleRights::RECEIVE).ok()?;

        if send_handle(SERVER, receiver, b"denied") != Err(Error::AccessDenied)
            || receive_handle(CLIENT, sender) != Err(Error::AccessDenied)
        {
            return None;
        }

        send_handle(CLIENT, sender, b"one").ok()?;
        send_handle(CLIENT, sender, b"two").ok()?;
        let first = receive_handle(SERVER, receiver).ok()?;
        let second = receive_handle(SERVER, receiver).ok()?;
        if first.sender != CLIENT
            || first.payload() != b"one"
            || second.sender != CLIENT
            || second.payload() != b"two"
            || receive_handle(SERVER, receiver) != Err(Error::QueueEmpty)
        {
            return None;
        }

        for index in 0..QUEUE_DEPTH {
            let byte = [index as u8];
            send_handle(CLIENT, sender, &byte).ok()?;
        }
        if send_handle(CLIENT, sender, b"overflow") != Err(Error::QueueFull) {
            return None;
        }
        for index in 0..QUEUE_DEPTH {
            let message = receive_handle(SERVER, receiver).ok()?;
            if message.sender != CLIENT || message.payload() != [index as u8] {
                return None;
            }
        }

        if close_handle(CLIENT, sender).is_err()
            || send_handle(CLIENT, sender, b"stale") != Err(Error::InvalidHandle)
            || object_count() != baseline_objects + 1
        {
            return None;
        }

        Some(())
    })()
    .is_some();

    let server_clean = unregister(SERVER).is_ok();
    let client_clean = unregister(CLIENT).is_ok();
    result && server_clean && client_clean && object_count() == baseline_objects
}

const STAGE8_3_SERVER: u64 = u64::MAX - 24;
const STAGE8_3_CLIENT: u64 = u64::MAX - 25;
static STAGE8_3_OWNER_HANDLE: AtomicU32 = AtomicU32::new(0);
static STAGE8_3_SEND_HANDLE: AtomicU32 = AtomicU32::new(0);
static STAGE8_3_RECEIVE_HANDLE: AtomicU32 = AtomicU32::new(0);
static STAGE8_3_PHASE1_OK: AtomicBool = AtomicBool::new(false);
static STAGE8_3_FULL_READY: AtomicBool = AtomicBool::new(false);
static STAGE8_3_RECEIVER_DONE: AtomicBool = AtomicBool::new(false);
static STAGE8_3_RECEIVER_OK: AtomicBool = AtomicBool::new(false);
static STAGE8_3_SENDER_DONE: AtomicBool = AtomicBool::new(false);
static STAGE8_3_SENDER_OK: AtomicBool = AtomicBool::new(false);

fn stage8_3_owner_handle() -> Handle {
    Handle(STAGE8_3_OWNER_HANDLE.load(Ordering::Acquire))
}

fn stage8_3_send_handle() -> Handle {
    Handle(STAGE8_3_SEND_HANDLE.load(Ordering::Acquire))
}

fn stage8_3_receive_handle() -> Handle {
    Handle(STAGE8_3_RECEIVE_HANDLE.load(Ordering::Acquire))
}

fn stage8_3_finish_receiver(ok: bool) -> ! {
    STAGE8_3_RECEIVER_OK.store(ok, Ordering::Release);
    STAGE8_3_RECEIVER_DONE.store(true, Ordering::Release);
    task::exit_current_task()
}

fn stage8_3_finish_sender(ok: bool) -> ! {
    STAGE8_3_SENDER_OK.store(ok, Ordering::Release);
    STAGE8_3_SENDER_DONE.store(true, Ordering::Release);
    task::exit_current_task()
}

fn stage8_3_receiver_task() -> ! {
    let receiver = stage8_3_receive_handle();
    let owner = stage8_3_owner_handle();

    let Ok(first) = receive_handle_blocking(STAGE8_3_SERVER, receiver) else {
        stage8_3_finish_receiver(false);
    };
    if first.sender != STAGE8_3_CLIENT || first.payload() != b"wake-before-block-safe" {
        stage8_3_finish_receiver(false);
    }
    STAGE8_3_PHASE1_OK.store(true, Ordering::Release);

    while !STAGE8_3_FULL_READY.load(Ordering::Acquire) {
        task::yield_now();
    }

    // Do not free queue capacity until the sender is observably registered as
    // a writable waiter. This proves the full-queue path truly blocks and is
    // later released by a dequeue event rather than simply winning a race.
    loop {
        match waiter_counts(STAGE8_3_SERVER, owner) {
            Ok((_, send_waiters)) if send_waiters != 0 => break,
            Ok(_) => task::yield_now(),
            Err(_) => stage8_3_finish_receiver(false),
        }
    }

    let Ok(head) = receive_handle(STAGE8_3_SERVER, receiver) else {
        stage8_3_finish_receiver(false);
    };
    if head.sender != STAGE8_3_CLIENT || head.payload() != [0] {
        stage8_3_finish_receiver(false);
    }

    for index in 1..QUEUE_DEPTH {
        let Ok(message) = receive_handle_blocking(STAGE8_3_SERVER, receiver) else {
            stage8_3_finish_receiver(false);
        };
        if message.sender != STAGE8_3_CLIENT || message.payload() != [index as u8] {
            stage8_3_finish_receiver(false);
        }
    }

    let Ok(tail) = receive_handle_blocking(STAGE8_3_SERVER, receiver) else {
        stage8_3_finish_receiver(false);
    };
    if tail.sender != STAGE8_3_CLIENT || tail.payload() != b"after-full" {
        stage8_3_finish_receiver(false);
    }

    stage8_3_finish_receiver(true)
}

fn stage8_3_sender_task() -> ! {
    let sender = stage8_3_send_handle();
    let owner = stage8_3_owner_handle();

    // Wait until the receiver has registered its empty-queue wait. The actual
    // scheduler state may still be Running/Ready here; signal_event must handle
    // either side of that boundary without losing the event.
    loop {
        match waiter_counts(STAGE8_3_SERVER, owner) {
            Ok((receive_waiters, _)) if receive_waiters != 0 => break,
            Ok(_) => task::yield_now(),
            Err(_) => stage8_3_finish_sender(false),
        }
    }

    if send_handle(STAGE8_3_CLIENT, sender, b"wake-before-block-safe").is_err() {
        stage8_3_finish_sender(false);
    }

    while !STAGE8_3_PHASE1_OK.load(Ordering::Acquire) {
        task::yield_now();
    }

    for index in 0..QUEUE_DEPTH {
        let byte = [index as u8];
        if send_handle(STAGE8_3_CLIENT, sender, &byte).is_err() {
            stage8_3_finish_sender(false);
        }
    }
    STAGE8_3_FULL_READY.store(true, Ordering::Release);

    if send_handle_blocking(STAGE8_3_CLIENT, sender, b"after-full").is_err() {
        stage8_3_finish_sender(false);
    }

    stage8_3_finish_sender(true)
}

/// Stage 8.3 production runtime proof for race-free wait/wake semantics.
///
/// Phase 1 blocks a receiver on an empty endpoint and wakes it from a peer
/// task. Phase 2 fills the endpoint, blocks the sender, then frees one slot and
/// proves the sender resumes without losing FIFO order. On SMP the sender runs
/// on the highest online AP so the same event protocol crosses CPUs.
pub fn stage8_3_runtime_probe() -> bool {
    STAGE8_3_OWNER_HANDLE.store(0, Ordering::Release);
    STAGE8_3_SEND_HANDLE.store(0, Ordering::Release);
    STAGE8_3_RECEIVE_HANDLE.store(0, Ordering::Release);
    STAGE8_3_PHASE1_OK.store(false, Ordering::Release);
    STAGE8_3_FULL_READY.store(false, Ordering::Release);
    STAGE8_3_RECEIVER_DONE.store(false, Ordering::Release);
    STAGE8_3_RECEIVER_OK.store(false, Ordering::Release);
    STAGE8_3_SENDER_DONE.store(false, Ordering::Release);
    STAGE8_3_SENDER_OK.store(false, Ordering::Release);

    if register(STAGE8_3_SERVER).is_err() || register(STAGE8_3_CLIENT).is_err() {
        let _ = unregister(STAGE8_3_SERVER);
        let _ = unregister(STAGE8_3_CLIENT);
        return false;
    }

    let baseline_objects = object_count();
    let setup = (|| {
        let owner = create_endpoint_object(STAGE8_3_SERVER).ok()?;
        let sender =
            grant_handle(STAGE8_3_SERVER, owner, STAGE8_3_CLIENT, HandleRights::SEND).ok()?;
        let receiver = grant_handle(
            STAGE8_3_SERVER,
            owner,
            STAGE8_3_SERVER,
            HandleRights::RECEIVE,
        )
        .ok()?;
        STAGE8_3_OWNER_HANDLE.store(owner.as_u32(), Ordering::Release);
        STAGE8_3_SEND_HANDLE.store(sender.as_u32(), Ordering::Release);
        STAGE8_3_RECEIVE_HANDLE.store(receiver.as_u32(), Ordering::Release);
        Some(())
    })()
    .is_some();

    if !setup {
        let _ = unregister(STAGE8_3_SERVER);
        let _ = unregister(STAGE8_3_CLIENT);
        return false;
    }

    if task::spawn("s8.3-ipc-receiver", stage8_3_receiver_task).is_err() {
        return false;
    }

    let sender_spawned = if crate::smp::online_count() > 1 {
        // SAFETY: this probe touches only atomics, the SMP-safe IPC spin lock,
        // and scheduler yield/wait/exit primitives. It does not enter BSP-only
        // allocator, paging, storage, network, userspace or GUI services.
        unsafe {
            task::spawn_on(
                crate::smp::online_count() - 1,
                "s8.3-ipc-sender",
                stage8_3_sender_task,
            )
        }
        .is_ok()
    } else {
        task::spawn("s8.3-ipc-sender", stage8_3_sender_task).is_ok()
    };
    if !sender_spawned {
        return false;
    }

    let deadline = crate::timer::ticks().saturating_add(512);
    while !(STAGE8_3_RECEIVER_DONE.load(Ordering::Acquire)
        && STAGE8_3_SENDER_DONE.load(Ordering::Acquire))
    {
        if crate::timer::ticks() >= deadline {
            return false;
        }
        task::yield_now();
    }

    let owner = stage8_3_owner_handle();
    let receiver = stage8_3_receive_handle();
    let clean_waiters = waiter_counts(STAGE8_3_SERVER, owner) == Ok((0, 0));
    let queue_empty = receive_handle(STAGE8_3_SERVER, receiver) == Err(Error::QueueEmpty);
    let workers_ok =
        STAGE8_3_RECEIVER_OK.load(Ordering::Acquire) && STAGE8_3_SENDER_OK.load(Ordering::Acquire);

    let server_clean = unregister(STAGE8_3_SERVER).is_ok();
    let client_clean = unregister(STAGE8_3_CLIENT).is_ok();

    workers_ok
        && clean_waiters
        && queue_empty
        && server_clean
        && client_clean
        && object_count() == baseline_objects
}

const STAGE8_4_OWNER: u64 = u64::MAX - 32;
const STAGE8_4_PEER: u64 = u64::MAX - 33;
const STAGE8_4_ADDRESS: u64 = 0x0000_4000_0000;
const STAGE8_4_PAGES: usize = 2;

fn stage8_4_drop_mapping_record_after_space_destroy(
    owner: u64,
    space: paging::AddressSpace,
    start: u64,
) -> bool {
    let taken = STATE.lock().take_shared_mapping(owner, space, start);
    let Ok((object_ref, _, _)) = taken else {
        return false;
    };
    STATE.lock().cancel_shared_mapping(object_ref);
    true
}

/// Stage 8.4 boot-time production proof for shared-memory object semantics.
///
/// The probe creates one two-page object, grants a read/map-only handle to a
/// peer, maps the same physical pages into two independent user page tables,
/// proves cross-page visibility, enforces WRITE rights, exercises explicit
/// unmap/remap, then closes every handle while mappings remain. Mapping-held
/// references keep the object alive until the two address spaces are destroyed.
pub fn stage8_4_runtime_probe() -> bool {
    if register(STAGE8_4_OWNER).is_err() || register(STAGE8_4_PEER).is_err() {
        let _ = unregister(STAGE8_4_OWNER);
        let _ = unregister(STAGE8_4_PEER);
        return false;
    }

    let baseline_objects = object_count();
    let baseline_shared = shared_memory_count();
    let baseline_frames = crate::memory::stats().allocated_frames;

    let Some(owner_space) = paging::create_user_address_space(STAGE8_4_ADDRESS) else {
        let _ = unregister(STAGE8_4_OWNER);
        let _ = unregister(STAGE8_4_PEER);
        return false;
    };
    let Some(peer_space) = paging::create_user_address_space(STAGE8_4_ADDRESS) else {
        let _ = paging::discard_empty_user_address_space(owner_space);
        let _ = unregister(STAGE8_4_OWNER);
        let _ = unregister(STAGE8_4_PEER);
        return false;
    };

    let setup = (|| {
        let owner = create_shared_memory(STAGE8_4_OWNER, STAGE8_4_PAGES).ok()?;
        let peer = grant_handle(
            STAGE8_4_OWNER,
            owner,
            STAGE8_4_PEER,
            HandleRights::SHM_READ | HandleRights::SHM_MAP | HandleRights::INSPECT,
        )
        .ok()?;

        let owner_info = shared_memory_info(STAGE8_4_OWNER, owner).ok()?;
        let peer_info = shared_memory_info(STAGE8_4_PEER, peer).ok()?;
        if owner_info.page_count != STAGE8_4_PAGES
            || peer_info.object_id != owner_info.object_id
            || owner_info.mapping_count != 0
            || peer_info.mapping_count != 0
            || handle_info(STAGE8_4_OWNER, owner).ok()?.kind != HandleObjectKind::SharedMemory
        {
            return None;
        }

        if write_shared_memory(STAGE8_4_PEER, peer, 0, b"denied") != Err(Error::AccessDenied)
            || grant_handle(STAGE8_4_OWNER, owner, STAGE8_4_PEER, HandleRights::SEND)
                != Err(Error::AccessDenied)
            || map_shared_memory(STAGE8_4_PEER, peer, peer_space, STAGE8_4_ADDRESS, true)
                != Err(Error::AccessDenied)
        {
            return None;
        }

        let size = STAGE8_4_PAGES * 4096;
        if map_shared_memory(STAGE8_4_OWNER, owner, owner_space, STAGE8_4_ADDRESS, true).ok()?
            != size
            || map_shared_memory(STAGE8_4_PEER, peer, peer_space, STAGE8_4_ADDRESS, false).ok()?
                != size
        {
            return None;
        }

        if !paging::user_range_has_protection_in(owner_space, STAGE8_4_ADDRESS, size, true, false)
            || !paging::user_range_has_protection_in(
                peer_space,
                STAGE8_4_ADDRESS,
                size,
                false,
                false,
            )
        {
            return None;
        }

        for page in 0..STAGE8_4_PAGES {
            let address = STAGE8_4_ADDRESS + (page * 4096) as u64;
            if paging::user_frame_in(owner_space, address)
                != paging::user_frame_in(peer_space, address)
            {
                return None;
            }
        }

        // Deliberately cross the 4 KiB boundary to prove both pages belong to
        // the same shared object and are visible through independent page tables.
        let cross_page = b"wovenhat-shared-memory";
        let cross_start = STAGE8_4_ADDRESS + 4090;
        paging::write_user_bytes(owner_space, cross_start, cross_page).ok()?;
        let mut observed = [0_u8; 22];
        paging::read_user_bytes_in(peer_space, cross_start, &mut observed).ok()?;
        if observed != *cross_page {
            return None;
        }

        write_shared_memory(STAGE8_4_OWNER, owner, 64, b"kernel-object-write").ok()?;
        let mut direct_observed = [0_u8; 19];
        read_shared_memory(STAGE8_4_OWNER, owner, 64, &mut direct_observed).ok()?;
        if direct_observed != *b"kernel-object-write" {
            return None;
        }

        let mut object_observed = [0_u8; 19];
        paging::read_user_bytes_in(peer_space, STAGE8_4_ADDRESS + 64, &mut object_observed).ok()?;
        if object_observed != *b"kernel-object-write" {
            return None;
        }

        // Exercise the public unmap transaction, then remap the same address so
        // final address-space destruction can reclaim the page-table hierarchy.
        unmap_shared_memory(STAGE8_4_OWNER, owner_space, STAGE8_4_ADDRESS).ok()?;
        if !paging::user_range_is_unmapped_in(owner_space, STAGE8_4_ADDRESS, size)
            || shared_memory_info(STAGE8_4_OWNER, owner)
                .ok()?
                .mapping_count
                != 1
        {
            return None;
        }
        map_shared_memory(STAGE8_4_OWNER, owner, owner_space, STAGE8_4_ADDRESS, true).ok()?;

        // Handles may disappear while mappings remain. The mapping references,
        // not the creator handle, own the backing lifetime from this point.
        close_handle(STAGE8_4_OWNER, owner).ok()?;
        close_handle(STAGE8_4_PEER, peer).ok()?;
        if shared_memory_count() != baseline_shared + 1
            || unregister(STAGE8_4_OWNER) != Err(Error::MappingBusy)
        {
            return None;
        }

        let mut after_close = [0_u8; 22];
        paging::read_user_bytes_in(peer_space, cross_start, &mut after_close).ok()?;
        if after_close != *cross_page {
            return None;
        }

        Some(())
    })()
    .is_some();

    let size = STAGE8_4_PAGES * 4096;
    let owner_destroyed =
        paging::destroy_user_address_space(owner_space, &[(STAGE8_4_ADDRESS, size)]).is_ok();
    let owner_record = owner_destroyed
        && stage8_4_drop_mapping_record_after_space_destroy(
            STAGE8_4_OWNER,
            owner_space,
            STAGE8_4_ADDRESS,
        );
    let peer_destroyed =
        paging::destroy_user_address_space(peer_space, &[(STAGE8_4_ADDRESS, size)]).is_ok();
    let peer_record = peer_destroyed
        && stage8_4_drop_mapping_record_after_space_destroy(
            STAGE8_4_PEER,
            peer_space,
            STAGE8_4_ADDRESS,
        );

    let owner_clean = unregister(STAGE8_4_OWNER).is_ok();
    let peer_clean = unregister(STAGE8_4_PEER).is_ok();

    setup
        && owner_record
        && peer_record
        && owner_clean
        && peer_clean
        && shared_memory_count() == baseline_shared
        && object_count() == baseline_objects
        && crate::memory::stats().allocated_frames == baseline_frames
}

const STAGE9_2D_OWNER: u64 = u64::MAX - 80;
const STAGE9_2D_CLIENT: u64 = u64::MAX - 81;
const STAGE9_2D_SERVICE: &[u8] = b"woven.stage9_2d";

// -------------------------------------------------------------------------
// Stage 9.2E: SMP revocation stress and lineage closure.
// -------------------------------------------------------------------------
const STAGE9_2E_OWNER: u64 = u64::MAX - 0x92e0;
const STAGE9_2E_WORKER_BASE: u64 = u64::MAX - 0x92f0;
const STAGE9_2E_POST_REVOKE_CHECKS: usize = 32;
const STAGE9_2E_REUSE_ROUNDS: usize = 24;

static STAGE9_2E_RELEASE: AtomicBool = AtomicBool::new(false);
static STAGE9_2E_REVOKE_STARTED: AtomicBool = AtomicBool::new(false);
static STAGE9_2E_REVOKED: AtomicBool = AtomicBool::new(false);
static STAGE9_2E_READY: AtomicUsize = AtomicUsize::new(0);
static STAGE9_2E_DONE: AtomicUsize = AtomicUsize::new(0);
static STAGE9_2E_OK: AtomicUsize = AtomicUsize::new(0);
static STAGE9_2E_CPU_MASK: AtomicUsize = AtomicUsize::new(0);
static STAGE9_2E_HANDLES: [AtomicU32; crate::smp::MAX_CPUS] =
    [const { AtomicU32::new(0) }; crate::smp::MAX_CPUS];
static STAGE9_2E_PRE_USES: [AtomicUsize; crate::smp::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; crate::smp::MAX_CPUS];
static STAGE9_2E_POST_DENIALS: [AtomicUsize; crate::smp::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; crate::smp::MAX_CPUS];
static STAGE9_2E_FAIL: [AtomicUsize; crate::smp::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; crate::smp::MAX_CPUS];

const fn stage9_2e_worker_owner(cpu: usize) -> u64 {
    STAGE9_2E_WORKER_BASE - cpu as u64
}

fn stage9_2e_fail(cpu: usize, code: usize) {
    let _ = STAGE9_2E_FAIL[cpu].compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire);
}

fn stage9_2e_worker_task() -> ! {
    let cpu = crate::smp::cpu_index();
    let owner = stage9_2e_worker_owner(cpu);
    let handle = Handle(STAGE9_2E_HANDLES[cpu].load(Ordering::Acquire));
    let mut ok = handle.as_u32() != 0;
    if !ok {
        stage9_2e_fail(cpu, 1);
    }

    STAGE9_2E_CPU_MASK.fetch_or(1usize << cpu, Ordering::AcqRel);
    STAGE9_2E_READY.fetch_add(1, Ordering::AcqRel);
    while !STAGE9_2E_RELEASE.load(Ordering::Acquire) {
        task::yield_now();
    }

    // Exercise live delegated authority until revocation becomes visible.
    // AccessDenied is permitted only once the coordinator has begun recall.
    let mut observed = [0u8; 4];
    while ok && !STAGE9_2E_REVOKED.load(Ordering::Acquire) {
        match read_shared_memory(owner, handle, 0, &mut observed) {
            Ok(()) if observed == *b"9.2E" => {
                STAGE9_2E_PRE_USES[cpu].fetch_add(1, Ordering::AcqRel);
            }
            Ok(()) => {
                stage9_2e_fail(cpu, 2);
                ok = false;
            }
            Err(Error::AccessDenied) if STAGE9_2E_REVOKE_STARTED.load(Ordering::Acquire) => {
                task::yield_now();
            }
            Err(_) => {
                stage9_2e_fail(cpu, 3);
                ok = false;
            }
        }
        task::yield_now();
    }

    // Once revoke_object_delegations() has returned, no operation may
    // successfully use the stale delegated handle on any CPU.
    for _ in 0..STAGE9_2E_POST_REVOKE_CHECKS {
        if !ok {
            break;
        }
        if read_shared_memory(owner, handle, 0, &mut observed) != Err(Error::AccessDenied)
            || handle_info(owner, handle) != Err(Error::AccessDenied)
        {
            stage9_2e_fail(cpu, 4);
            ok = false;
            break;
        }
        STAGE9_2E_POST_DENIALS[cpu].fetch_add(1, Ordering::AcqRel);
        task::yield_now();
    }

    // Revocation invalidates authority, not handle-table bookkeeping. Closing a
    // revoked handle must remain legal so process teardown cannot leak objects.
    if close_handle(owner, handle).is_err() {
        stage9_2e_fail(cpu, 5);
        ok = false;
    }
    if unregister(owner).is_err() {
        stage9_2e_fail(cpu, 6);
        ok = false;
    }

    if ok {
        STAGE9_2E_OK.fetch_add(1, Ordering::AcqRel);
    }
    STAGE9_2E_DONE.fetch_add(1, Ordering::Release);
    task::exit_current_task();
}

fn stage9_2e_dump_state(label: &str, online: usize, baseline_lineages: usize) {
    crate::serial::write_line(format_args!(
        "[S9.2E][DIAG] {} online={} ready={} done={} ok={} mask={:#x} revoke_started={} revoked={} lineages={}/{}",
        label,
        online,
        STAGE9_2E_READY.load(Ordering::Acquire),
        STAGE9_2E_DONE.load(Ordering::Acquire),
        STAGE9_2E_OK.load(Ordering::Acquire),
        STAGE9_2E_CPU_MASK.load(Ordering::Acquire),
        STAGE9_2E_REVOKE_STARTED.load(Ordering::Acquire),
        STAGE9_2E_REVOKED.load(Ordering::Acquire),
        wovenguard::lineage_count(),
        baseline_lineages,
    ));
    for cpu in 0..online {
        crate::serial::write_line(format_args!(
            "[S9.2E][DIAG] cpu={} handle={:#x} pre={} denied={} fail={}",
            cpu,
            STAGE9_2E_HANDLES[cpu].load(Ordering::Acquire),
            STAGE9_2E_PRE_USES[cpu].load(Ordering::Acquire),
            STAGE9_2E_POST_DENIALS[cpu].load(Ordering::Acquire),
            STAGE9_2E_FAIL[cpu].load(Ordering::Acquire),
        ));
    }
}

/// Stage 9.2E production proof. Every online CPU concurrently exercises the
/// same delegated shared-memory authority while the object owner recursively
/// revokes its WovenGuard lineage. The returned revocation is the linearization
/// point: after it completes every worker must observe AccessDenied, yet each
/// revoked handle must remain closable. The second phase repeatedly reuses the
/// same handle slot across fresh delegation epochs to prove generation-tagged
/// stale handles cannot resurrect authority and that lineage slots do not leak.
pub fn stage9_2e_runtime_probe() -> bool {
    let online = crate::smp::online_count();
    if online == 0 || online > crate::smp::MAX_CPUS {
        return false;
    }

    let baseline_objects = object_count();
    let baseline_shared = shared_memory_count();
    let baseline_services = service_count();
    let baseline_lineages = wovenguard::lineage_count();

    STAGE9_2E_RELEASE.store(false, Ordering::Release);
    STAGE9_2E_REVOKE_STARTED.store(false, Ordering::Release);
    STAGE9_2E_REVOKED.store(false, Ordering::Release);
    STAGE9_2E_READY.store(0, Ordering::Release);
    STAGE9_2E_DONE.store(0, Ordering::Release);
    STAGE9_2E_OK.store(0, Ordering::Release);
    STAGE9_2E_CPU_MASK.store(0, Ordering::Release);
    for cpu in 0..crate::smp::MAX_CPUS {
        STAGE9_2E_HANDLES[cpu].store(0, Ordering::Release);
        STAGE9_2E_PRE_USES[cpu].store(0, Ordering::Release);
        STAGE9_2E_POST_DENIALS[cpu].store(0, Ordering::Release);
        STAGE9_2E_FAIL[cpu].store(0, Ordering::Release);
    }

    let setup = (|| -> Option<Handle> {
        register(STAGE9_2E_OWNER).ok()?;
        for cpu in 0..online {
            register(stage9_2e_worker_owner(cpu)).ok()?;
        }
        let shared = create_shared_memory(STAGE9_2E_OWNER, 1).ok()?;
        write_shared_memory(STAGE9_2E_OWNER, shared, 0, b"9.2E").ok()?;
        for (cpu, slot) in STAGE9_2E_HANDLES.iter().enumerate().take(online) {
            let delegated = grant_handle(
                STAGE9_2E_OWNER,
                shared,
                stage9_2e_worker_owner(cpu),
                HandleRights::SHM_READ | HandleRights::INSPECT,
            )
            .ok()?;
            slot.store(delegated.as_u32(), Ordering::Release);
        }
        for cpu in 0..online {
            // SAFETY: workers access only SMP-safe atomics, scheduler yield/exit,
            // and IPC/shared-memory operations already validated for AP use.
            if unsafe { task::spawn_on(cpu, "s9.2e-revoke-worker", stage9_2e_worker_task) }.is_err()
            {
                return None;
            }
        }
        Some(shared)
    })();

    let Some(shared) = setup else {
        stage9_2e_dump_state("setup-failed", online, baseline_lineages);
        return false;
    };

    let ready_deadline = crate::timer::ticks().saturating_add(512);
    while STAGE9_2E_READY.load(Ordering::Acquire) < online {
        if crate::timer::ticks() >= ready_deadline {
            stage9_2e_dump_state("ready-timeout", online, baseline_lineages);
            return false;
        }
        task::yield_now();
    }
    STAGE9_2E_RELEASE.store(true, Ordering::Release);

    // Require every CPU to prove that the delegated lineage was usable before
    // recall. This prevents a vacuous post-revocation-only stress pass.
    let live_deadline = crate::timer::ticks().saturating_add(1024);
    loop {
        if STAGE9_2E_PRE_USES
            .iter()
            .take(online)
            .all(|count| count.load(Ordering::Acquire) != 0)
        {
            break;
        }
        if crate::timer::ticks() >= live_deadline {
            stage9_2e_dump_state("pre-revoke-timeout", online, baseline_lineages);
            return false;
        }
        task::yield_now();
    }

    STAGE9_2E_REVOKE_STARTED.store(true, Ordering::Release);
    if revoke_object_delegations(STAGE9_2E_OWNER, shared) != Ok(1) {
        stage9_2e_dump_state("revoke-failed", online, baseline_lineages);
        return false;
    }
    STAGE9_2E_REVOKED.store(true, Ordering::Release);

    let completion_deadline = crate::timer::ticks().saturating_add(2048);
    while STAGE9_2E_DONE.load(Ordering::Acquire) < online {
        if crate::timer::ticks() >= completion_deadline {
            stage9_2e_dump_state("completion-timeout", online, baseline_lineages);
            return false;
        }
        task::yield_now();
    }

    let workers_ok = STAGE9_2E_OK.load(Ordering::Acquire) == online
        && STAGE9_2E_CPU_MASK.load(Ordering::Acquire) == (1usize << online) - 1
        && STAGE9_2E_PRE_USES
            .iter()
            .take(online)
            .all(|count| count.load(Ordering::Acquire) != 0)
        && STAGE9_2E_POST_DENIALS
            .iter()
            .take(online)
            .all(|count| count.load(Ordering::Acquire) == STAGE9_2E_POST_REVOKE_CHECKS)
        && STAGE9_2E_FAIL
            .iter()
            .take(online)
            .all(|code| code.load(Ordering::Acquire) == 0);
    if !workers_ok {
        stage9_2e_dump_state("worker-validation-failed", online, baseline_lineages);
        return false;
    }

    // The intrinsic owner authority survives delegated-tree recall.
    let mut seed = [0u8; 4];
    if read_shared_memory(STAGE9_2E_OWNER, shared, 0, &mut seed).is_err() || seed != *b"9.2E" {
        stage9_2e_dump_state("owner-authority-failed", online, baseline_lineages);
        return false;
    }

    // Repeatedly create, revoke, close, and recreate a delegated handle in the
    // same process. A stale userspace handle must become InvalidHandle after
    // slot reuse, while the fresh generation remains valid until its own epoch
    // is revoked. This also repeatedly reuses lineage slots.
    let mut previous: Option<Handle> = None;
    for _ in 0..STAGE9_2E_REUSE_ROUNDS {
        let fresh = match grant_handle(
            STAGE9_2E_OWNER,
            shared,
            STAGE9_2E_OWNER,
            HandleRights::SHM_READ | HandleRights::INSPECT,
        ) {
            Ok(handle) => handle,
            Err(_) => return false,
        };
        if let Some(stale) = previous {
            if handle_info(STAGE9_2E_OWNER, stale) != Err(Error::InvalidHandle) {
                return false;
            }
        }
        if read_shared_memory(STAGE9_2E_OWNER, fresh, 0, &mut seed).is_err()
            || revoke_object_delegations(STAGE9_2E_OWNER, shared) != Ok(1)
            || read_shared_memory(STAGE9_2E_OWNER, fresh, 0, &mut seed) != Err(Error::AccessDenied)
            || close_handle(STAGE9_2E_OWNER, fresh).is_err()
            || wovenguard::lineage_count() != baseline_lineages
        {
            return false;
        }
        previous = Some(fresh);
    }

    let cleanup =
        close_handle(STAGE9_2E_OWNER, shared).is_ok() && unregister(STAGE9_2E_OWNER).is_ok();
    let closed = cleanup
        && object_count() == baseline_objects
        && shared_memory_count() == baseline_shared
        && service_count() == baseline_services
        && wovenguard::lineage_count() == baseline_lineages;
    if !closed {
        stage9_2e_dump_state("cleanup-failed", online, baseline_lineages);
    }
    closed
}

/// Stage 9.2D production proof: IPC object delegation is bound to a WovenGuard
/// lineage. Revoking the object's delegation root invalidates direct grants,
/// queued-transfer receivers and service-discovery handles immediately while
/// preserving the creator's intrinsic owner authority and object lifetime.
pub fn stage9_2d_runtime_probe() -> bool {
    let baseline_lineages = wovenguard::lineage_count();
    if register(STAGE9_2D_OWNER).is_err() || register(STAGE9_2D_CLIENT).is_err() {
        let _ = unregister(STAGE9_2D_OWNER);
        let _ = unregister(STAGE9_2D_CLIENT);
        return false;
    }

    let result = (|| {
        // Direct endpoint grant: delegated SEND becomes unusable after recall,
        // while the creator's intrinsic owner handle remains valid.
        let endpoint = create_endpoint_object(STAGE9_2D_OWNER).ok()?;
        let delegated = grant_handle(
            STAGE9_2D_OWNER,
            endpoint,
            STAGE9_2D_CLIENT,
            HandleRights::SEND | HandleRights::INSPECT,
        )
        .ok()?;
        if handle_info(STAGE9_2D_CLIENT, delegated).is_err() {
            return None;
        }
        if revoke_object_delegations(STAGE9_2D_OWNER, endpoint).ok()? != 1
            || send_handle(STAGE9_2D_CLIENT, delegated, b"revoked") != Err(Error::AccessDenied)
            || handle_info(STAGE9_2D_OWNER, endpoint).is_err()
        {
            return None;
        }
        close_handle(STAGE9_2D_CLIENT, delegated).ok()?;
        close_handle(STAGE9_2D_OWNER, endpoint).ok()?;

        // Queued transfer escrow: the fresh receiver-local handle carries the
        // source object's lineage rather than laundering authority through IPC.
        let channel = create_endpoint_object(STAGE9_2D_OWNER).ok()?;
        let shared = create_shared_memory(STAGE9_2D_OWNER, 1).ok()?;
        write_shared_memory(STAGE9_2D_OWNER, shared, 0, b"lineaged").ok()?;
        send_handle_with_transfer(
            STAGE9_2D_OWNER,
            channel,
            shared,
            HandleRights::SHM_READ | HandleRights::INSPECT,
            b"lineaged-transfer",
        )
        .ok()?;
        let message = receive_handle(STAGE9_2D_OWNER, channel).ok()?;
        let received = message.transferred_handle()?;
        let mut bytes = [0_u8; 8];
        read_shared_memory(STAGE9_2D_OWNER, received, 0, &mut bytes).ok()?;
        if bytes != *b"lineaged" {
            return None;
        }
        if revoke_object_delegations(STAGE9_2D_OWNER, shared).ok()? != 1
            || read_shared_memory(STAGE9_2D_OWNER, received, 0, &mut bytes)
                != Err(Error::AccessDenied)
            || read_shared_memory(STAGE9_2D_OWNER, shared, 0, &mut bytes).is_err()
        {
            return None;
        }
        close_handle(STAGE9_2D_OWNER, received).ok()?;
        close_handle(STAGE9_2D_OWNER, shared).ok()?;
        close_handle(STAGE9_2D_OWNER, channel).ok()?;

        // Service discovery is also delegation: a revoked publisher object can
        // no longer mint fresh clients, and already-discovered clients go stale.
        let service = create_endpoint_object(STAGE9_2D_OWNER).ok()?;
        publish_service(
            STAGE9_2D_OWNER,
            STAGE9_2D_SERVICE,
            service,
            HandleRights::SEND | HandleRights::INSPECT,
        )
        .ok()?;
        let client = discover_service(STAGE9_2D_CLIENT, STAGE9_2D_SERVICE).ok()?;
        if revoke_object_delegations(STAGE9_2D_OWNER, service).ok()? != 1
            || send_handle(STAGE9_2D_CLIENT, client, b"revoked-service") != Err(Error::AccessDenied)
            || discover_service(STAGE9_2D_CLIENT, STAGE9_2D_SERVICE) != Err(Error::AccessDenied)
        {
            return None;
        }
        unpublish_service(STAGE9_2D_OWNER, STAGE9_2D_SERVICE).ok()?;
        close_handle(STAGE9_2D_CLIENT, client).ok()?;
        close_handle(STAGE9_2D_OWNER, service).ok()?;
        Some(())
    })()
    .is_some();

    let clean = unregister(STAGE9_2D_CLIENT).is_ok()
        && unregister(STAGE9_2D_OWNER).is_ok()
        && wovenguard::lineage_count() == baseline_lineages;
    result && clean
}

const STAGE8_5_SERVER: u64 = u64::MAX - 40;
const STAGE8_5_CLIENT: u64 = u64::MAX - 41;

/// Stage 8.5 production proof: capabilities are escrowed by object identity,
/// survive sender-handle closure, are installed as fresh receiver-local handles,
/// cannot gain rights, and remain queued atomically if the receiver handle table
/// is full. Both endpoint and shared-memory object transfer are exercised.
pub fn stage8_5_runtime_probe() -> bool {
    if register(STAGE8_5_SERVER).is_err() || register(STAGE8_5_CLIENT).is_err() {
        let _ = unregister(STAGE8_5_SERVER);
        let _ = unregister(STAGE8_5_CLIENT);
        return false;
    }

    let baseline_objects = object_count();
    let baseline_shared = shared_memory_count();

    let result = (|| {
        let channel_owner = create_endpoint_object(STAGE8_5_SERVER).ok()?;
        let channel_send = grant_handle(
            STAGE8_5_SERVER,
            channel_owner,
            STAGE8_5_CLIENT,
            HandleRights::SEND,
        )
        .ok()?;
        let channel_receive = grant_handle(
            STAGE8_5_SERVER,
            channel_owner,
            STAGE8_5_SERVER,
            HandleRights::RECEIVE | HandleRights::INSPECT,
        )
        .ok()?;

        let shared = create_shared_memory(STAGE8_5_CLIENT, 1).ok()?;
        write_shared_memory(STAGE8_5_CLIENT, shared, 0, b"stage8.5-escrow").ok()?;

        // Rights amplification and self-transfer are rejected before enqueue.
        if send_handle_with_transfer(
            STAGE8_5_CLIENT,
            channel_send,
            shared,
            HandleRights::SHARED_MEMORY_OWNER | HandleRights::SEND,
            b"bad-rights",
        ) != Err(Error::AccessDenied)
        {
            return None;
        }
        if send_handle_with_transfer(
            STAGE8_5_SERVER,
            channel_owner,
            channel_owner,
            HandleRights::SEND,
            b"self-cycle",
        ) != Err(Error::AccessDenied)
        {
            return None;
        }

        send_handle_with_transfer(
            STAGE8_5_CLIENT,
            channel_send,
            shared,
            HandleRights::SHM_READ | HandleRights::SHM_MAP | HandleRights::INSPECT,
            b"shared-capability",
        )
        .ok()?;

        // The escrow reference must keep the object alive after the sender
        // closes its last shared-memory handle.
        close_handle(STAGE8_5_CLIENT, shared).ok()?;
        if shared_memory_count() != baseline_shared + 1 {
            return None;
        }

        let received = receive_handle(STAGE8_5_SERVER, channel_receive).ok()?;
        if received.payload() != b"shared-capability" {
            return None;
        }
        let received_shared = received.transferred_handle()?;
        let info = handle_info(STAGE8_5_SERVER, received_shared).ok()?;
        if info.kind != HandleObjectKind::SharedMemory
            || info.rights
                != (HandleRights::SHM_READ | HandleRights::SHM_MAP | HandleRights::INSPECT)
            || write_shared_memory(STAGE8_5_SERVER, received_shared, 0, b"denied")
                != Err(Error::AccessDenied)
        {
            return None;
        }
        let mut observed = [0_u8; 15];
        read_shared_memory(STAGE8_5_SERVER, received_shared, 0, &mut observed).ok()?;
        if observed != *b"stage8.5-escrow" {
            return None;
        }
        close_handle(STAGE8_5_SERVER, received_shared).ok()?;
        if shared_memory_count() != baseline_shared {
            return None;
        }

        // Transfer an endpoint capability as well. This proves object-kind
        // neutrality while still enforcing endpoint-specific rights.
        let secondary = create_endpoint_object(STAGE8_5_CLIENT).ok()?;
        send_handle_with_transfer(
            STAGE8_5_CLIENT,
            channel_send,
            secondary,
            HandleRights::SEND | HandleRights::INSPECT,
            b"endpoint-capability",
        )
        .ok()?;
        close_handle(STAGE8_5_CLIENT, secondary).ok()?;
        let endpoint_message = receive_handle(STAGE8_5_SERVER, channel_receive).ok()?;
        let received_endpoint = endpoint_message.transferred_handle()?;
        let endpoint_info = handle_info(STAGE8_5_SERVER, received_endpoint).ok()?;
        if endpoint_message.payload() != b"endpoint-capability"
            || endpoint_info.kind != HandleObjectKind::Endpoint
            || endpoint_info.rights != (HandleRights::SEND | HandleRights::INSPECT)
        {
            return None;
        }
        close_handle(STAGE8_5_SERVER, received_endpoint).ok()?;

        // Queue teardown must release escrow references even if nobody receives
        // the capability-bearing message. This prevents ordinary abandoned
        // endpoint destruction from leaking transferred objects.
        let abandoned_channel = create_endpoint_object(STAGE8_5_CLIENT).ok()?;
        let abandoned_shared = create_shared_memory(STAGE8_5_CLIENT, 1).ok()?;
        send_handle_with_transfer(
            STAGE8_5_CLIENT,
            abandoned_channel,
            abandoned_shared,
            HandleRights::SHM_READ,
            b"abandoned",
        )
        .ok()?;
        close_handle(STAGE8_5_CLIENT, abandoned_shared).ok()?;
        if shared_memory_count() != baseline_shared + 1 {
            return None;
        }
        close_handle(STAGE8_5_CLIENT, abandoned_channel).ok()?;
        if shared_memory_count() != baseline_shared {
            return None;
        }

        // A full destination queue must roll back the temporary escrow retain.
        let full_channel = create_endpoint_object(STAGE8_5_CLIENT).ok()?;
        for index in 0..QUEUE_DEPTH {
            send_handle(STAGE8_5_CLIENT, full_channel, &[index as u8]).ok()?;
        }
        let rollback_shared = create_shared_memory(STAGE8_5_CLIENT, 1).ok()?;
        if send_handle_with_transfer(
            STAGE8_5_CLIENT,
            full_channel,
            rollback_shared,
            HandleRights::SHM_READ,
            b"must-not-queue",
        ) != Err(Error::QueueFull)
        {
            return None;
        }
        close_handle(STAGE8_5_CLIENT, rollback_shared).ok()?;
        if shared_memory_count() != baseline_shared {
            return None;
        }
        close_handle(STAGE8_5_CLIENT, full_channel).ok()?;

        // Fill the receiver handle table, then prove receive failure is atomic:
        // the capability-bearing message stays queued and can be received after
        // one slot is released.
        let transfer_again = create_shared_memory(STAGE8_5_CLIENT, 1).ok()?;
        send_handle_with_transfer(
            STAGE8_5_CLIENT,
            channel_send,
            transfer_again,
            HandleRights::SHM_READ | HandleRights::INSPECT,
            b"atomic-full-table",
        )
        .ok()?;
        close_handle(STAGE8_5_CLIENT, transfer_again).ok()?;

        let mut fillers = [None; MAX_HANDLES];
        let mut fill_len = 0usize;
        while handle_count(STAGE8_5_SERVER) < MAX_HANDLES {
            let filler = create_endpoint_object(STAGE8_5_SERVER).ok()?;
            fillers[fill_len] = Some(filler);
            fill_len += 1;
        }
        if receive_handle(STAGE8_5_SERVER, channel_receive) != Err(Error::HandleTableFull) {
            return None;
        }
        let released = fillers[..fill_len]
            .iter_mut()
            .rev()
            .find_map(|slot| slot.take())?;
        close_handle(STAGE8_5_SERVER, released).ok()?;
        let retried = receive_handle(STAGE8_5_SERVER, channel_receive).ok()?;
        if retried.payload() != b"atomic-full-table" {
            return None;
        }
        let retried_handle = retried.transferred_handle()?;
        close_handle(STAGE8_5_SERVER, retried_handle).ok()?;
        for filler in fillers.into_iter().flatten() {
            close_handle(STAGE8_5_SERVER, filler).ok()?;
        }

        close_handle(STAGE8_5_CLIENT, channel_send).ok()?;
        close_handle(STAGE8_5_SERVER, channel_receive).ok()?;
        close_handle(STAGE8_5_SERVER, channel_owner).ok()?;
        Some(())
    })()
    .is_some();

    let clean = unregister(STAGE8_5_CLIENT).is_ok()
        && unregister(STAGE8_5_SERVER).is_ok()
        && object_count() == baseline_objects
        && shared_memory_count() == baseline_shared;
    result && clean
}

const STAGE8_6_SERVER: u64 = u64::MAX - 50;
const STAGE8_6_CLIENT: u64 = u64::MAX - 51;
const STAGE8_6_OTHER: u64 = u64::MAX - 52;

/// Stage 8.6 production probe: named service publication/discovery with
/// registry-held lifetime, exact client-rights delegation, namespace
/// uniqueness, access control, and owner-exit cleanup.
pub fn stage8_6_runtime_probe() -> bool {
    let _ = unregister(STAGE8_6_SERVER);
    let _ = unregister(STAGE8_6_CLIENT);
    let _ = unregister(STAGE8_6_OTHER);
    let baseline_objects = object_count();
    let baseline_services = service_count();

    let result = (|| -> Option<()> {
        register(STAGE8_6_SERVER).ok()?;
        register(STAGE8_6_CLIENT).ok()?;
        register(STAGE8_6_OTHER).ok()?;

        let service = create_endpoint_object(STAGE8_6_SERVER).ok()?;
        let service_info_before = handle_info(STAGE8_6_SERVER, service).ok()?;

        // Names are bounded and deterministic.
        if publish_service(STAGE8_6_SERVER, b"", service, HandleRights::SEND)
            != Err(Error::InvalidServiceName)
        {
            return None;
        }
        if publish_service(STAGE8_6_SERVER, b"woven/bad", service, HandleRights::SEND)
            != Err(Error::InvalidServiceName)
        {
            return None;
        }

        // Discovery may only mint client-side endpoint rights. RECEIVE and
        // PUBLISH_SERVICE remain server authority.
        if publish_service(
            STAGE8_6_SERVER,
            b"woven.echo",
            service,
            HandleRights::RECEIVE,
        ) != Err(Error::AccessDenied)
        {
            return None;
        }
        // Possessing an endpoint is not enough to register it as a service.
        // Publication is its own capability bit.
        let restricted_publish = grant_handle(
            STAGE8_6_SERVER,
            service,
            STAGE8_6_CLIENT,
            HandleRights::SEND | HandleRights::INSPECT,
        )
        .ok()?;
        if publish_service(
            STAGE8_6_CLIENT,
            b"woven.denied",
            restricted_publish,
            HandleRights::SEND,
        ) != Err(Error::AccessDenied)
        {
            return None;
        }
        close_handle(STAGE8_6_CLIENT, restricted_publish).ok()?;

        publish_service(
            STAGE8_6_SERVER,
            b"woven.echo",
            service,
            HandleRights::SEND | HandleRights::INSPECT,
        )
        .ok()?;

        let advertised = service_info(b"woven.echo").ok()?;
        if advertised.owner != STAGE8_6_SERVER
            || advertised.object_id != service_info_before.object_id
            || advertised.client_rights != (HandleRights::SEND | HandleRights::INSPECT)
            || advertised.name_length != b"woven.echo".len()
            || service_count() != baseline_services + 1
        {
            return None;
        }

        // Global names are unique even when another valid publisher owns a
        // different endpoint.
        let other_endpoint = create_endpoint_object(STAGE8_6_OTHER).ok()?;
        if publish_service(
            STAGE8_6_OTHER,
            b"woven.echo",
            other_endpoint,
            HandleRights::SEND,
        ) != Err(Error::ServiceExists)
        {
            return None;
        }
        close_handle(STAGE8_6_OTHER, other_endpoint).ok()?;

        let client_handle = discover_service(STAGE8_6_CLIENT, b"woven.echo").ok()?;
        let client_info = handle_info(STAGE8_6_CLIENT, client_handle).ok()?;
        if client_info.object_id != service_info_before.object_id
            || client_info.rights != (HandleRights::SEND | HandleRights::INSPECT)
            || receive_handle(STAGE8_6_CLIENT, client_handle) != Err(Error::AccessDenied)
        {
            return None;
        }
        send_handle(STAGE8_6_CLIENT, client_handle, b"service-discovery").ok()?;
        let request = receive_handle(STAGE8_6_SERVER, service).ok()?;
        if request.sender != STAGE8_6_CLIENT || request.payload() != b"service-discovery" {
            return None;
        }
        close_handle(STAGE8_6_CLIENT, client_handle).ok()?;

        // Registry escrow keeps the service object alive after the source
        // handle closes. A later client discovery still succeeds.
        close_handle(STAGE8_6_SERVER, service).ok()?;
        let rediscovered = discover_service(STAGE8_6_CLIENT, b"woven.echo").ok()?;
        let rediscovered_info = handle_info(STAGE8_6_CLIENT, rediscovered).ok()?;
        if rediscovered_info.object_id != service_info_before.object_id {
            return None;
        }
        close_handle(STAGE8_6_CLIENT, rediscovered).ok()?;

        if unpublish_service(STAGE8_6_CLIENT, b"woven.echo") != Err(Error::AccessDenied) {
            return None;
        }
        unpublish_service(STAGE8_6_SERVER, b"woven.echo").ok()?;
        if discover_service(STAGE8_6_CLIENT, b"woven.echo") != Err(Error::ServiceNotFound)
            || service_count() != baseline_services
        {
            return None;
        }

        // Owner teardown must automatically remove registrations and release
        // their escrow references, even if the publisher already closed its
        // endpoint handle.
        let exit_service = create_endpoint_object(STAGE8_6_SERVER).ok()?;
        publish_service(
            STAGE8_6_SERVER,
            b"woven.exit",
            exit_service,
            HandleRights::SEND,
        )
        .ok()?;
        close_handle(STAGE8_6_SERVER, exit_service).ok()?;
        unregister(STAGE8_6_SERVER).ok()?;
        if service_info(b"woven.exit") != Err(Error::ServiceNotFound)
            || discover_service(STAGE8_6_CLIENT, b"woven.exit") != Err(Error::ServiceNotFound)
            || service_count() != baseline_services
        {
            return None;
        }

        unregister(STAGE8_6_CLIENT).ok()?;
        unregister(STAGE8_6_OTHER).ok()?;
        Some(())
    })();

    if result.is_none() {
        let _ = unregister(STAGE8_6_SERVER);
        let _ = unregister(STAGE8_6_CLIENT);
        let _ = unregister(STAGE8_6_OTHER);
        return false;
    }

    object_count() == baseline_objects && service_count() == baseline_services
}

// Stage 8.7: integrated multicore IPC stress. One producer is pinned to every
// online CPU while a blocking receiver drains a single service endpoint. The
// workers repeatedly discover the service, exercise blocking queue backpressure,
// transfer reduced-rights shared-memory capabilities, exercise deterministic
// queue saturation, close transient handles, and tear down IPC namespaces.
/// Stage 9.4B production proof for named-service exposure policy.
/// Uses the real idle TCB profile plus the real service registry, then restores
/// the validated Stage 9.4A Restricted profile and IPC namespace.
pub fn stage9_4b_runtime_probe() -> bool {
    const OWNER: u64 = 1;
    const SERVICE: &[u8] = b"sys.net.stage94b";
    const DENIED: &[u8] = b"sys.ai.stage94b";

    let owner_task = TaskId::from_u64(OWNER);
    let baseline_services = service_count();
    let baseline_objects = object_count();
    let baseline = task::task_sandbox_profile(owner_task);
    if baseline != Some(wovenguard::SandboxProfile::RESTRICTED) {
        return false;
    }

    let profile =
        wovenguard::SandboxProfile::new(0x94b0, CapabilitySet::only(Capability::TimerRead))
            .with_service_policy(wovenguard::ServicePolicy::only(
                ServiceClass::Network,
                ServiceClass::Network,
            ));
    if task::bind_sandbox_profile(owner_task, profile).is_err() {
        return false;
    }

    let result = (|| -> Option<bool> {
        register(OWNER).ok()?;
        let endpoint = create_endpoint_object(OWNER).ok()?;
        publish_service(OWNER, SERVICE, endpoint, HandleRights::SERVICE_CLIENT).ok()?;
        if publish_service(OWNER, DENIED, endpoint, HandleRights::SERVICE_CLIENT)
            != Err(Error::AccessDenied)
        {
            return Some(false);
        }
        let client = discover_service(OWNER, SERVICE).ok()?;
        if send_handle(OWNER, client, b"9.4B").is_err() {
            return Some(false);
        }
        if discover_service(OWNER, DENIED) != Err(Error::AccessDenied) {
            return Some(false);
        }

        let tightened =
            wovenguard::SandboxProfile::new(0x94b1, CapabilitySet::only(Capability::TimerRead))
                .with_service_policy(wovenguard::ServicePolicy::NONE);
        if task::bind_sandbox_profile(owner_task, tightened).is_err() {
            return Some(false);
        }
        if send_handle(OWNER, client, b"blocked") != Err(Error::AccessDenied) {
            return Some(false);
        }
        if !crate::audit::latest().is_some_and(|event| {
            event.actor == OWNER
                && event.action == crate::audit::Action::SandboxServiceUse
                && !event.allowed
        }) {
            return Some(false);
        }
        Some(true)
    })()
    .unwrap_or(false);

    let _ = unpublish_service(OWNER, SERVICE);
    let _ = unregister(OWNER);
    let restored =
        task::bind_sandbox_profile(owner_task, wovenguard::SandboxProfile::RESTRICTED).is_ok();

    result
        && restored
        && task::task_sandbox_profile(owner_task) == baseline
        && service_count() == baseline_services
        && object_count() == baseline_objects
}

const STAGE8_7_SERVER: u64 = u64::MAX - 64;
const STAGE8_7_WORKER_BASE: u64 = u64::MAX - 80;
const STAGE8_7_ROUNDS: usize = 32;
const STAGE8_7_SERVICE: &[u8] = b"woven.smp-stress";
const STAGE8_7_TRANSFER_RIGHTS: u8 = HandleRights::SHM_READ | HandleRights::INSPECT;

static STAGE8_7_RELEASE: AtomicBool = AtomicBool::new(false);
static STAGE8_7_STRESS_START: AtomicBool = AtomicBool::new(false);
static STAGE8_7_RECEIVER_DONE: AtomicBool = AtomicBool::new(false);
static STAGE8_7_RECEIVER_OK: AtomicBool = AtomicBool::new(false);
static STAGE8_7_WORKERS_READY: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_WORKERS_DONE: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_WORKERS_OK: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_CPU_MASK: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_QUEUE_FULL_HITS: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_PREFILL_SENT: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_PREFILL_DRAINED: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_MESSAGES_SENT: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_MESSAGES_RECEIVED: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_TRANSFERS_SENT: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_TRANSFERS_RECEIVED: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_RECEIVER_FAIL: AtomicUsize = AtomicUsize::new(0);
static STAGE8_7_WORKER_FAIL: [AtomicUsize; crate::smp::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; crate::smp::MAX_CPUS];
static STAGE8_7_RECEIVED_PER_CPU: [AtomicUsize; crate::smp::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; crate::smp::MAX_CPUS];
static STAGE8_7_SEEN_ROUNDS: [AtomicU32; crate::smp::MAX_CPUS] =
    [const { AtomicU32::new(0) }; crate::smp::MAX_CPUS];
static STAGE8_7_SERVER_HANDLE: AtomicU32 = AtomicU32::new(0);
static STAGE8_7_SHARED_OBJECT: AtomicU64 = AtomicU64::new(0);
static STAGE8_7_SHM_HANDLES: [AtomicU32; crate::smp::MAX_CPUS] =
    [const { AtomicU32::new(0) }; crate::smp::MAX_CPUS];

const fn stage8_7_worker_owner(cpu: usize) -> u64 {
    STAGE8_7_WORKER_BASE - cpu as u64
}

fn stage8_7_set_worker_fail(cpu: usize, code: usize) {
    if cpu < crate::smp::MAX_CPUS {
        let _ = STAGE8_7_WORKER_FAIL[cpu].compare_exchange(
            0,
            code,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

fn stage8_7_validate_transfer(handle: Handle, expected_object: u64) -> bool {
    let info = match handle_info(STAGE8_7_SERVER, handle) {
        Ok(info) => info,
        Err(_) => return false,
    };
    let mut shared_seed = [0u8; 4];
    info.kind == HandleObjectKind::SharedMemory
        && info.object_id.as_u64() == expected_object
        && info.rights == STAGE8_7_TRANSFER_RIGHTS
        && read_shared_memory(STAGE8_7_SERVER, handle, 0, &mut shared_seed).is_ok()
        && shared_seed == *b"S8.7"
        && close_handle(STAGE8_7_SERVER, handle).is_ok()
}

fn stage8_7_worker_task() -> ! {
    let cpu = crate::smp::cpu_index();
    let owner = stage8_7_worker_owner(cpu);
    let shared = Handle(STAGE8_7_SHM_HANDLES[cpu].load(Ordering::Acquire));
    let mut ok = shared.as_u32() != 0;
    if !ok {
        stage8_7_set_worker_fail(cpu, 1);
    }
    STAGE8_7_CPU_MASK.fetch_or(1usize << cpu, Ordering::AcqRel);

    STAGE8_7_WORKERS_READY.fetch_add(1, Ordering::AcqRel);
    while !STAGE8_7_RELEASE.load(Ordering::Acquire) {
        task::yield_now();
    }

    // Deterministic saturation phase. Only CPU0 fills the endpoint with
    // capability-bearing messages while the receiver intentionally waits.
    // This guarantees QueueFull is reached without allowing ordinary blocking
    // sends to fill the queue first and deadlock the stress harness itself.
    if ok && cpu == 0 {
        let endpoint = match discover_service(owner, STAGE8_7_SERVICE) {
            Ok(handle) => handle,
            Err(_) => {
                stage8_7_set_worker_fail(cpu, 2);
                ok = false;
                Handle(0)
            }
        };
        if ok {
            let mut sequence = 0u8;
            loop {
                let payload = [0xf7, sequence, 1, 0x87];
                match send_handle_with_transfer(
                    owner,
                    endpoint,
                    shared,
                    STAGE8_7_TRANSFER_RIGHTS,
                    &payload,
                ) {
                    Ok(()) => {
                        STAGE8_7_PREFILL_SENT.fetch_add(1, Ordering::AcqRel);
                        sequence = sequence.wrapping_add(1);
                    }
                    Err(Error::QueueFull) => {
                        STAGE8_7_QUEUE_FULL_HITS.fetch_add(1, Ordering::AcqRel);
                        break;
                    }
                    Err(_) => {
                        stage8_7_set_worker_fail(cpu, 3);
                        ok = false;
                        break;
                    }
                }
            }
            if close_handle(owner, endpoint).is_err() {
                stage8_7_set_worker_fail(cpu, 4);
                ok = false;
            }
        }
    }

    while ok && !STAGE8_7_STRESS_START.load(Ordering::Acquire) {
        task::yield_now();
    }

    for round in 0..STAGE8_7_ROUNDS {
        if !ok {
            break;
        }

        let endpoint = match discover_service(owner, STAGE8_7_SERVICE) {
            Ok(handle) => handle,
            Err(_) => {
                stage8_7_set_worker_fail(cpu, 5);
                ok = false;
                break;
            }
        };
        let transfer = round.is_multiple_of(4);
        let payload = [cpu as u8, round as u8, transfer as u8, 0x87];

        if transfer {
            loop {
                match send_handle_with_transfer(
                    owner,
                    endpoint,
                    shared,
                    STAGE8_7_TRANSFER_RIGHTS,
                    &payload,
                ) {
                    Ok(()) => {
                        STAGE8_7_TRANSFERS_SENT.fetch_add(1, Ordering::AcqRel);
                        STAGE8_7_MESSAGES_SENT.fetch_add(1, Ordering::AcqRel);
                        break;
                    }
                    Err(Error::QueueFull) => {
                        STAGE8_7_QUEUE_FULL_HITS.fetch_add(1, Ordering::AcqRel);
                        task::yield_now();
                    }
                    Err(_) => {
                        stage8_7_set_worker_fail(cpu, 6);
                        ok = false;
                        break;
                    }
                }
            }
        } else if send_handle_blocking(owner, endpoint, &payload).is_err() {
            stage8_7_set_worker_fail(cpu, 7);
            ok = false;
        } else {
            STAGE8_7_MESSAGES_SENT.fetch_add(1, Ordering::AcqRel);
        }

        if close_handle(owner, endpoint).is_err() {
            stage8_7_set_worker_fail(cpu, 8);
            ok = false;
        }
        if !ok {
            break;
        }
        if round & 1 == 0 {
            task::yield_now();
        }
    }

    // Teardown races with the receiver draining messages. Capability-bearing
    // queue entries must keep their shared object alive after this closes the
    // workers' source handles and namespaces.
    if unregister(owner).is_err() {
        stage8_7_set_worker_fail(cpu, 9);
        ok = false;
    }
    if ok {
        STAGE8_7_WORKERS_OK.fetch_add(1, Ordering::AcqRel);
    }
    STAGE8_7_WORKERS_DONE.fetch_add(1, Ordering::Release);
    task::exit_current_task();
}

fn stage8_7_receiver_task() -> ! {
    let receiver = Handle(STAGE8_7_SERVER_HANDLE.load(Ordering::Acquire));
    let online = crate::smp::online_count();
    let expected_object = STAGE8_7_SHARED_OBJECT.load(Ordering::Acquire);
    let mut per_cpu = [0usize; crate::smp::MAX_CPUS];
    let mut seen_rounds = [0u32; crate::smp::MAX_CPUS];
    let mut ok = receiver.as_u32() != 0 && expected_object != 0;
    if !ok {
        STAGE8_7_RECEIVER_FAIL.store(1, Ordering::Release);
    }

    // CPU0's prefill phase guarantees a real QueueFull event using only
    // nonblocking capability-bearing sends. No ordinary sender can block before
    // this proof completes, eliminating the old stress-harness circular wait.
    while ok && STAGE8_7_QUEUE_FULL_HITS.load(Ordering::Acquire) == 0 {
        task::yield_now();
    }

    let prefill = STAGE8_7_PREFILL_SENT.load(Ordering::Acquire);
    for sequence in 0..prefill {
        let message = match receive_handle_blocking(STAGE8_7_SERVER, receiver) {
            Ok(message) => message,
            Err(_) => {
                STAGE8_7_RECEIVER_FAIL.store(2, Ordering::Release);
                ok = false;
                break;
            }
        };
        let payload = message.payload();
        if payload != [0xf7, sequence as u8, 1, 0x87] || message.sender != stage8_7_worker_owner(0)
        {
            STAGE8_7_RECEIVER_FAIL.store(3, Ordering::Release);
            ok = false;
            break;
        }
        match message.transferred_handle() {
            Some(handle) if stage8_7_validate_transfer(handle, expected_object) => {
                STAGE8_7_PREFILL_DRAINED.fetch_add(1, Ordering::AcqRel);
            }
            _ => {
                STAGE8_7_RECEIVER_FAIL.store(4, Ordering::Release);
                ok = false;
                break;
            }
        }
    }

    if ok {
        STAGE8_7_STRESS_START.store(true, Ordering::Release);
    }

    for _ in 0..online * STAGE8_7_ROUNDS {
        if !ok {
            break;
        }
        let message = match receive_handle_blocking(STAGE8_7_SERVER, receiver) {
            Ok(message) => message,
            Err(_) => {
                STAGE8_7_RECEIVER_FAIL.store(5, Ordering::Release);
                ok = false;
                break;
            }
        };
        if message.payload().len() != 4 || message.payload()[3] != 0x87 {
            STAGE8_7_RECEIVER_FAIL.store(6, Ordering::Release);
            ok = false;
            break;
        }
        let cpu = message.payload()[0] as usize;
        let round = message.payload()[1] as usize;
        let transfer = message.payload()[2] != 0;
        if cpu >= online
            || round >= STAGE8_7_ROUNDS
            || message.sender != stage8_7_worker_owner(cpu)
            || transfer != round.is_multiple_of(4)
        {
            STAGE8_7_RECEIVER_FAIL.store(7, Ordering::Release);
            ok = false;
            break;
        }
        let round_bit = 1u32 << round;
        if seen_rounds[cpu] & round_bit != 0 {
            STAGE8_7_RECEIVER_FAIL.store(8, Ordering::Release);
            ok = false;
            break;
        }
        seen_rounds[cpu] |= round_bit;
        per_cpu[cpu] += 1;
        STAGE8_7_RECEIVED_PER_CPU[cpu].store(per_cpu[cpu], Ordering::Release);
        STAGE8_7_SEEN_ROUNDS[cpu].store(seen_rounds[cpu], Ordering::Release);
        STAGE8_7_MESSAGES_RECEIVED.fetch_add(1, Ordering::AcqRel);

        match (transfer, message.transferred_handle()) {
            (true, Some(handle)) if stage8_7_validate_transfer(handle, expected_object) => {
                STAGE8_7_TRANSFERS_RECEIVED.fetch_add(1, Ordering::AcqRel);
            }
            (false, None) => {}
            _ => {
                STAGE8_7_RECEIVER_FAIL.store(9, Ordering::Release);
                ok = false;
                break;
            }
        }

        if per_cpu.iter().take(online).sum::<usize>() & 7 == 0 {
            task::yield_now();
        }
    }

    ok &= per_cpu
        .iter()
        .take(online)
        .all(|count| *count == STAGE8_7_ROUNDS)
        && seen_rounds
            .iter()
            .take(online)
            .all(|seen| *seen == u32::MAX);
    if !ok && STAGE8_7_RECEIVER_FAIL.load(Ordering::Acquire) == 0 {
        STAGE8_7_RECEIVER_FAIL.store(10, Ordering::Release);
    }
    STAGE8_7_RECEIVER_OK.store(ok, Ordering::Release);
    STAGE8_7_RECEIVER_DONE.store(true, Ordering::Release);
    task::exit_current_task();
}

fn stage8_7_dump_state(
    label: &str,
    online: usize,
    baseline_objects: usize,
    baseline_shared: usize,
    baseline_services: usize,
) {
    crate::serial::write_line(format_args!(
        "[S8.7][DIAG] {} online={} ready={} done={} ok={} mask={:#x} qfull={} prefill={}/{} sent={} recv={} xfer={}/{} receiver_done={} receiver_ok={} receiver_fail={}",
        label,
        online,
        STAGE8_7_WORKERS_READY.load(Ordering::Acquire),
        STAGE8_7_WORKERS_DONE.load(Ordering::Acquire),
        STAGE8_7_WORKERS_OK.load(Ordering::Acquire),
        STAGE8_7_CPU_MASK.load(Ordering::Acquire),
        STAGE8_7_QUEUE_FULL_HITS.load(Ordering::Acquire),
        STAGE8_7_PREFILL_DRAINED.load(Ordering::Acquire),
        STAGE8_7_PREFILL_SENT.load(Ordering::Acquire),
        STAGE8_7_MESSAGES_SENT.load(Ordering::Acquire),
        STAGE8_7_MESSAGES_RECEIVED.load(Ordering::Acquire),
        STAGE8_7_TRANSFERS_SENT.load(Ordering::Acquire),
        STAGE8_7_TRANSFERS_RECEIVED.load(Ordering::Acquire),
        STAGE8_7_RECEIVER_DONE.load(Ordering::Acquire),
        STAGE8_7_RECEIVER_OK.load(Ordering::Acquire),
        STAGE8_7_RECEIVER_FAIL.load(Ordering::Acquire),
    ));
    for cpu in 0..online {
        crate::serial::write_line(format_args!(
            "[S8.7][DIAG] cpu={} worker_fail={} received={} seen={:#010x}",
            cpu,
            STAGE8_7_WORKER_FAIL[cpu].load(Ordering::Acquire),
            STAGE8_7_RECEIVED_PER_CPU[cpu].load(Ordering::Acquire),
            STAGE8_7_SEEN_ROUNDS[cpu].load(Ordering::Acquire),
        ));
    }
    let endpoint = Handle(STAGE8_7_SERVER_HANDLE.load(Ordering::Acquire));
    match waiter_counts(STAGE8_7_SERVER, endpoint) {
        Ok((send_waiters, receive_waiters)) => crate::serial::write_line(format_args!(
            "[S8.7][DIAG] waiters send={} recv={} objects={}/{} shared={}/{} services={}/{} endpoint_count={}",
            send_waiters,
            receive_waiters,
            object_count(),
            baseline_objects,
            shared_memory_count(),
            baseline_shared,
            service_count(),
            baseline_services,
            endpoint_count(),
        )),
        Err(_) => crate::serial::write_line(format_args!(
            "[S8.7][DIAG] waiters unavailable objects={}/{} shared={}/{} services={}/{} endpoint_count={}",
            object_count(),
            baseline_objects,
            shared_memory_count(),
            baseline_shared,
            service_count(),
            baseline_services,
            endpoint_count(),
        )),
    }
}

/// Stage 8.7 production proof: concurrent multicore pressure across the complete
/// Stage 8 IPC stack. The test deliberately combines service discovery,
/// generation-tagged handles, deterministic queue saturation, blocking queue
/// backpressure, scheduler wakeups, capability escrow, shared-memory
/// transfer/lifetime and concurrent process teardown.
pub fn stage8_7_runtime_probe() -> bool {
    let online = crate::smp::online_count();
    if online == 0 || online > crate::smp::MAX_CPUS {
        return false;
    }

    let baseline_objects = object_count();
    let baseline_shared = shared_memory_count();
    let baseline_services = service_count();
    STAGE8_7_RELEASE.store(false, Ordering::Release);
    STAGE8_7_STRESS_START.store(false, Ordering::Release);
    STAGE8_7_RECEIVER_DONE.store(false, Ordering::Release);
    STAGE8_7_RECEIVER_OK.store(false, Ordering::Release);
    STAGE8_7_WORKERS_READY.store(0, Ordering::Release);
    STAGE8_7_WORKERS_DONE.store(0, Ordering::Release);
    STAGE8_7_WORKERS_OK.store(0, Ordering::Release);
    STAGE8_7_CPU_MASK.store(0, Ordering::Release);
    STAGE8_7_QUEUE_FULL_HITS.store(0, Ordering::Release);
    STAGE8_7_PREFILL_SENT.store(0, Ordering::Release);
    STAGE8_7_PREFILL_DRAINED.store(0, Ordering::Release);
    STAGE8_7_MESSAGES_SENT.store(0, Ordering::Release);
    STAGE8_7_MESSAGES_RECEIVED.store(0, Ordering::Release);
    STAGE8_7_TRANSFERS_SENT.store(0, Ordering::Release);
    STAGE8_7_TRANSFERS_RECEIVED.store(0, Ordering::Release);
    STAGE8_7_RECEIVER_FAIL.store(0, Ordering::Release);
    STAGE8_7_SERVER_HANDLE.store(0, Ordering::Release);
    STAGE8_7_SHARED_OBJECT.store(0, Ordering::Release);
    for cpu in 0..crate::smp::MAX_CPUS {
        STAGE8_7_SHM_HANDLES[cpu].store(0, Ordering::Release);
        STAGE8_7_WORKER_FAIL[cpu].store(0, Ordering::Release);
        STAGE8_7_RECEIVED_PER_CPU[cpu].store(0, Ordering::Release);
        STAGE8_7_SEEN_ROUNDS[cpu].store(0, Ordering::Release);
    }

    let setup = (|| -> Option<()> {
        register(STAGE8_7_SERVER).ok()?;
        for cpu in 0..online {
            register(stage8_7_worker_owner(cpu)).ok()?;
        }

        let endpoint = create_endpoint_object(STAGE8_7_SERVER).ok()?;
        publish_service(
            STAGE8_7_SERVER,
            STAGE8_7_SERVICE,
            endpoint,
            HandleRights::SEND | HandleRights::INSPECT,
        )
        .ok()?;
        let shared = create_shared_memory(STAGE8_7_SERVER, 1).ok()?;
        let shared_info = shared_memory_info(STAGE8_7_SERVER, shared).ok()?;
        write_shared_memory(STAGE8_7_SERVER, shared, 0, b"S8.7").ok()?;
        STAGE8_7_SERVER_HANDLE.store(endpoint.as_u32(), Ordering::Release);
        STAGE8_7_SHARED_OBJECT.store(shared_info.object_id.as_u64(), Ordering::Release);

        for (cpu, slot) in STAGE8_7_SHM_HANDLES.iter().enumerate().take(online) {
            let handle = grant_handle(
                STAGE8_7_SERVER,
                shared,
                stage8_7_worker_owner(cpu),
                HandleRights::TRANSFER | HandleRights::INSPECT | HandleRights::SHM_READ,
            )
            .ok()?;
            slot.store(handle.as_u32(), Ordering::Release);
        }

        if task::spawn("s8.7-ipc-receiver", stage8_7_receiver_task).is_err() {
            return None;
        }
        for cpu in 0..online {
            // SAFETY: workers use only atomics, the SMP-safe IPC state lock and
            // scheduler wait/yield primitives. Shared-memory capability transfer
            // manipulates IPC metadata only; APs do not enter the paging layer.
            // They also avoid storage, network, userspace, GUI, heap allocation,
            // and CPU-local pointers.
            if unsafe { task::spawn_on(cpu, "s8.7-ipc-worker", stage8_7_worker_task) }.is_err() {
                return None;
            }
        }
        Some(())
    })()
    .is_some();

    if !setup {
        stage8_7_dump_state(
            "setup-failed",
            online,
            baseline_objects,
            baseline_shared,
            baseline_services,
        );
        return false;
    }

    let ready_deadline = crate::timer::ticks().saturating_add(512);
    while STAGE8_7_WORKERS_READY.load(Ordering::Acquire) < online {
        if crate::timer::ticks() >= ready_deadline {
            stage8_7_dump_state(
                "ready-timeout",
                online,
                baseline_objects,
                baseline_shared,
                baseline_services,
            );
            return false;
        }
        task::yield_now();
    }
    STAGE8_7_RELEASE.store(true, Ordering::Release);

    let deadline = crate::timer::ticks().saturating_add(4096);
    while STAGE8_7_WORKERS_DONE.load(Ordering::Acquire) < online
        || !STAGE8_7_RECEIVER_DONE.load(Ordering::Acquire)
    {
        if crate::timer::ticks() >= deadline {
            stage8_7_dump_state(
                "completion-timeout",
                online,
                baseline_objects,
                baseline_shared,
                baseline_services,
            );
            return false;
        }
        task::yield_now();
    }

    let workers_ok = STAGE8_7_WORKERS_OK.load(Ordering::Acquire) == online;
    let cpu_mask_ok = STAGE8_7_CPU_MASK.load(Ordering::Acquire) == (1usize << online) - 1;
    let saturated = STAGE8_7_QUEUE_FULL_HITS.load(Ordering::Acquire) != 0;
    let prefill_ok = STAGE8_7_PREFILL_SENT.load(Ordering::Acquire) != 0
        && STAGE8_7_PREFILL_DRAINED.load(Ordering::Acquire)
            == STAGE8_7_PREFILL_SENT.load(Ordering::Acquire);
    let messages_ok = STAGE8_7_MESSAGES_SENT.load(Ordering::Acquire) == online * STAGE8_7_ROUNDS
        && STAGE8_7_MESSAGES_RECEIVED.load(Ordering::Acquire) == online * STAGE8_7_ROUNDS;
    let transfers_expected = online * (STAGE8_7_ROUNDS / 4);
    let transfers_ok = STAGE8_7_TRANSFERS_SENT.load(Ordering::Acquire) == transfers_expected
        && STAGE8_7_TRANSFERS_RECEIVED.load(Ordering::Acquire) == transfers_expected;
    let receiver_ok = STAGE8_7_RECEIVER_OK.load(Ordering::Acquire);
    let waiter_clean = Handle(STAGE8_7_SERVER_HANDLE.load(Ordering::Acquire));
    let waiters_ok = waiter_counts(STAGE8_7_SERVER, waiter_clean) == Ok((0, 0));
    let queue_empty = receive_handle(STAGE8_7_SERVER, waiter_clean) == Err(Error::QueueEmpty);

    // Worker namespaces and their source handles are already gone. The server
    // still owns its original shared-memory handle and service endpoint.
    let server_shared = {
        let state = STATE.lock();
        state
            .handle_space(STAGE8_7_SERVER)
            .and_then(|space| {
                space.entries.iter().flatten().find(|entry| {
                    entry.object.kind == HandleObjectKind::SharedMemory
                        && entry.object.id.as_u64()
                            == STAGE8_7_SHARED_OBJECT.load(Ordering::Acquire)
                })
            })
            .map(|entry| entry.object)
    };
    let shared_still_alive = server_shared.is_some();

    let unpublish_ok = unpublish_service(STAGE8_7_SERVER, STAGE8_7_SERVICE).is_ok();
    let unregister_ok = unregister(STAGE8_7_SERVER).is_ok();
    let cleanup_ok = object_count() == baseline_objects
        && shared_memory_count() == baseline_shared
        && service_count() == baseline_services;

    let ok = workers_ok
        && cpu_mask_ok
        && saturated
        && prefill_ok
        && messages_ok
        && transfers_ok
        && receiver_ok
        && waiters_ok
        && queue_empty
        && shared_still_alive
        && unpublish_ok
        && unregister_ok
        && cleanup_ok;
    if !ok {
        stage8_7_dump_state(
            "final-invariant",
            online,
            baseline_objects,
            baseline_shared,
            baseline_services,
        );
        crate::serial::write_line(format_args!(
            "[S8.7][DIAG] checks workers={} mask={} saturated={} prefill={} messages={} transfers={} receiver={} waiters={} queue_empty={} shared_alive={} unpublish={} unregister={} cleanup={}",
            workers_ok,
            cpu_mask_ok,
            saturated,
            prefill_ok,
            messages_ok,
            transfers_ok,
            receiver_ok,
            waiters_ok,
            queue_empty,
            shared_still_alive,
            unpublish_ok,
            unregister_ok,
            cleanup_ok,
        ));
    }
    ok
}
