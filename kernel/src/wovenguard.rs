use spin::Mutex;

use crate::capability::{Capability, CapabilitySet};

/// Stage 9.1 security domains.
///
/// Domains describe *what class of authority a task may ever hold*. They are
/// deliberately separate from `CapabilitySet`: the domain is the ceiling,
/// while the capability set is the task's current authority beneath that
/// ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecurityDomain {
    /// The bootstrap kernel. This is the only domain with an unrestricted
    /// capability ceiling.
    Kernel,
    /// Kernel-side services/workers. Privileged enough to provide OS services,
    /// but not eligible for raw interrupt or memory-inspection authority.
    SystemService,
    /// Normal Ring-3 applications.
    User,
    /// Minimal-authority infrastructure such as idle tasks.
    Restricted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecisionReason {
    Allowed,
    ActorLacksTaskControl,
    ActorLacksCapability,
    TargetDomainCeiling,
    SandboxCeiling,
    InvalidSandboxProfile,
    KernelDomainProtected,
    ServicePublishDenied,
    ServiceDiscoverDenied,
    DevicePolicyDenied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub reason: DecisionReason,
}

impl PolicyDecision {
    const fn allow() -> Self {
        Self {
            allowed: true,
            reason: DecisionReason::Allowed,
        }
    }

    const fn deny(reason: DecisionReason) -> Self {
        Self {
            allowed: false,
            reason,
        }
    }
}

/// Whether a capability is permitted to exist inside a domain at all.
///
/// This is WovenGuard's first least-privilege ceiling. Later Stage 9 work can
/// split these broad domains into signed service identities and finer policy,
/// but code outside WovenGuard should not bypass this decision point.
pub const fn domain_ceiling(domain: SecurityDomain) -> CapabilitySet {
    match domain {
        SecurityDomain::Kernel => CapabilitySet::kernel_bootstrap(),
        SecurityDomain::SystemService => CapabilitySet::empty()
            .with(Capability::Console)
            .with(Capability::TimerRead)
            .with(Capability::TaskInspect)
            .with(Capability::TaskControl)
            .with(Capability::DeviceIo)
            .with(Capability::FileRead)
            .with(Capability::FileWrite)
            .with(Capability::Ipc)
            .with(Capability::ProcessCreate)
            .with(Capability::NetworkIo)
            .with(Capability::StorageIo)
            .with(Capability::DisplayIo)
            .with(Capability::InputIo),
        SecurityDomain::User => CapabilitySet::userspace(),
        SecurityDomain::Restricted => CapabilitySet::only(Capability::TimerRead),
    }
}

pub const fn domain_allows(domain: SecurityDomain, capability: Capability) -> bool {
    domain_ceiling(domain).contains(capability)
}

/// Stage 9.4A sandbox identity plus a capability ceiling beneath the broader
/// Stage 9.1 security-domain ceiling. Profiles are immutable values and remain
/// allocation-free so they can live directly in every TCB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ServiceClass {
    Core = 0,
    Network = 1,
    Storage = 2,
    Ui = 3,
    Ai = 4,
    Application = 5,
    Test = 6,
}

impl ServiceClass {
    const COUNT: u16 = 7;

    const fn bit(self) -> u16 {
        1 << self as u8
    }
}

/// Allocation-free publish/discovery policy for named service classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServicePolicy {
    publish_mask: u16,
    discover_mask: u16,
}

impl ServicePolicy {
    pub const ALL: Self = Self {
        publish_mask: (1 << ServiceClass::COUNT) - 1,
        discover_mask: (1 << ServiceClass::COUNT) - 1,
    };

    pub const NONE: Self = Self {
        publish_mask: 0,
        discover_mask: 0,
    };

    pub const fn only(publish: ServiceClass, discover: ServiceClass) -> Self {
        Self {
            publish_mask: publish.bit(),
            discover_mask: discover.bit(),
        }
    }

    pub const fn allows_publish(self, class: ServiceClass) -> bool {
        self.publish_mask & class.bit() != 0
    }

    pub const fn allows_discover(self, class: ServiceClass) -> bool {
        self.discover_mask & class.bit() != 0
    }
}

/// Stage 9.4B classifies names into stable policy buckets without allocation.
/// Existing unclassified application services remain in `Application`.
pub fn classify_service(name: &[u8]) -> ServiceClass {
    if name.starts_with(b"sys.net.") || name.starts_with(b"net.") {
        ServiceClass::Network
    } else if name.starts_with(b"sys.storage.") || name.starts_with(b"storage.") {
        ServiceClass::Storage
    } else if name.starts_with(b"sys.ui.") || name.starts_with(b"ui.") {
        ServiceClass::Ui
    } else if name.starts_with(b"sys.ai.") || name.starts_with(b"ai.") {
        ServiceClass::Ai
    } else if name.starts_with(b"sys.") {
        ServiceClass::Core
    } else if name.starts_with(b"test.") || name.starts_with(b"woven.test.") {
        ServiceClass::Test
    } else {
        ServiceClass::Application
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FileScope {
    Root = 0,
    System = 1,
    Temporary = 2,
    Mounted = 3,
    Home = 4,
    Other = 5,
}

impl FileScope {
    const COUNT: u16 = 6;

    const fn bit(self) -> u16 {
        1 << self as u8
    }
}

/// Allocation-free filesystem scope policy.  FileRead/FileWrite remain the
/// coarse capability gates; these masks decide which filesystem regions a
/// sandbox may use beneath those capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilePolicy {
    read_mask: u16,
    write_mask: u16,
}

impl FilePolicy {
    pub const ALL: Self = Self {
        read_mask: (1 << FileScope::COUNT) - 1,
        write_mask: (1 << FileScope::COUNT) - 1,
    };

    pub const NONE: Self = Self {
        read_mask: 0,
        write_mask: 0,
    };

    pub const fn scoped(read_mask: u16, write_mask: u16) -> Self {
        Self { read_mask, write_mask }
    }

    pub const fn scope_mask(scope: FileScope) -> u16 {
        scope.bit()
    }

    pub const fn allows_read(self, scope: FileScope) -> bool {
        self.read_mask & scope.bit() != 0
    }

    pub const fn allows_write(self, scope: FileScope) -> bool {
        self.write_mask & scope.bit() != 0
    }
}

/// Classify a canonical absolute path into a stable filesystem policy bucket.
/// Path normalization remains owned by task/VFS code; WovenGuard only receives
/// resolved absolute paths.
pub fn classify_file_path(path: &str) -> FileScope {
    if path == "/" {
        FileScope::Root
    } else if path == "/etc" || path.starts_with("/etc/") || path == "/bin" || path.starts_with("/bin/") {
        FileScope::System
    } else if path == "/tmp" || path.starts_with("/tmp/") {
        FileScope::Temporary
    } else if path == "/mnt" || path.starts_with("/mnt/") {
        FileScope::Mounted
    } else if path == "/home" || path.starts_with("/home/") {
        FileScope::Home
    } else {
        FileScope::Other
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DeviceClass {
    Network = 0,
    Storage = 1,
    Display = 2,
    Input = 3,
    Generic = 4,
}

impl DeviceClass {
    const COUNT: u16 = 5;

    const fn bit(self) -> u16 {
        1 << self as u8
    }
}

/// Allocation-free Stage 9.5 resource/device exposure mask.  A task must pass
/// both this sandbox-class gate and the class-specific capability gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevicePolicy {
    mask: u16,
}

impl DevicePolicy {
    pub const ALL: Self = Self { mask: (1 << DeviceClass::COUNT) - 1 };
    pub const NONE: Self = Self { mask: 0 };

    pub const fn only(class: DeviceClass) -> Self {
        Self { mask: class.bit() }
    }

    pub const fn allows(self, class: DeviceClass) -> bool {
        self.mask & class.bit() != 0
    }
}

pub const fn device_capability(class: DeviceClass) -> Capability {
    match class {
        DeviceClass::Network => Capability::NetworkIo,
        DeviceClass::Storage => Capability::StorageIo,
        DeviceClass::Display => Capability::DisplayIo,
        DeviceClass::Input => Capability::InputIo,
        DeviceClass::Generic => Capability::DeviceIo,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SandboxProfile {
    id: u32,
    ceiling: CapabilitySet,
    services: ServicePolicy,
    files: FilePolicy,
    devices: DevicePolicy,
}

impl SandboxProfile {
    pub const KERNEL_TRUSTED: Self = Self::new(1, CapabilitySet::kernel_bootstrap());
    pub const SYSTEM_SERVICE: Self = Self::new(2, domain_ceiling(SecurityDomain::SystemService));
    pub const USER_DEFAULT: Self = Self::new(3, CapabilitySet::userspace());
    pub const RESTRICTED: Self = Self::new(4, CapabilitySet::only(Capability::TimerRead));

    pub const fn new(id: u32, ceiling: CapabilitySet) -> Self {
        Self {
            id,
            ceiling,
            services: ServicePolicy::ALL,
            files: FilePolicy::ALL,
            devices: DevicePolicy::ALL,
        }
    }

    pub const fn with_service_policy(mut self, services: ServicePolicy) -> Self {
        self.services = services;
        self
    }

    pub const fn with_file_policy(mut self, files: FilePolicy) -> Self {
        self.files = files;
        self
    }

    pub const fn with_device_policy(mut self, devices: DevicePolicy) -> Self {
        self.devices = devices;
        self
    }

    pub const fn id(self) -> u32 {
        self.id
    }

    pub const fn allows(self, capability: Capability) -> bool {
        self.ceiling.contains(capability)
    }

    pub const fn allows_service_publish(self, class: ServiceClass) -> bool {
        self.services.allows_publish(class)
    }

    pub const fn allows_service_discover(self, class: ServiceClass) -> bool {
        self.services.allows_discover(class)
    }

    pub const fn allows_file_read(self, scope: FileScope) -> bool {
        self.files.allows_read(scope)
    }

    pub const fn allows_file_write(self, scope: FileScope) -> bool {
        self.files.allows_write(scope)
    }

    pub const fn allows_device(self, class: DeviceClass) -> bool {
        self.devices.allows(class)
    }

    pub const fn is_valid_for(self, domain: SecurityDomain) -> bool {
        self.id != 0 && self.ceiling.is_subset_of(domain_ceiling(domain))
    }
}

pub const fn default_sandbox(domain: SecurityDomain) -> SandboxProfile {
    match domain {
        SecurityDomain::Kernel => SandboxProfile::KERNEL_TRUSTED,
        SecurityDomain::SystemService => SandboxProfile::SYSTEM_SERVICE,
        SecurityDomain::User => SandboxProfile::USER_DEFAULT,
        SecurityDomain::Restricted => SandboxProfile::RESTRICTED,
    }
}

/// Stage 9.5 device/resource decision.  Device classes are intentionally
/// separate from service names: services mediate discovery, while this gate
/// decides whether the calling task may touch the underlying resource class.
pub fn authorize_device_access(
    authority: CapabilitySet,
    sandbox: SandboxProfile,
    class: DeviceClass,
) -> PolicyDecision {
    if !authority.contains(device_capability(class)) {
        return PolicyDecision::deny(DecisionReason::ActorLacksCapability);
    }
    if !sandbox.allows_device(class) {
        return PolicyDecision::deny(DecisionReason::DevicePolicyDenied);
    }
    PolicyDecision::allow()
}

/// Central Stage 9.1/9.4A authorization point for capability delegation.
///
/// The actor must already own both TaskControl and the capability being
/// delegated, and the target's domain ceiling must permit the capability.
/// Nobody outside the Kernel domain may mint authority into the Kernel domain.
pub fn authorize_grant(
    actor_domain: SecurityDomain,
    actor_capabilities: CapabilitySet,
    target_domain: SecurityDomain,
    target_sandbox: SandboxProfile,
    capability: Capability,
) -> PolicyDecision {
    if !actor_capabilities.contains(Capability::TaskControl) {
        return PolicyDecision::deny(DecisionReason::ActorLacksTaskControl);
    }
    if !actor_capabilities.contains(capability) {
        return PolicyDecision::deny(DecisionReason::ActorLacksCapability);
    }
    if target_domain == SecurityDomain::Kernel && actor_domain != SecurityDomain::Kernel {
        return PolicyDecision::deny(DecisionReason::KernelDomainProtected);
    }
    if !domain_allows(target_domain, capability) {
        return PolicyDecision::deny(DecisionReason::TargetDomainCeiling);
    }
    if !target_sandbox.is_valid_for(target_domain) {
        return PolicyDecision::deny(DecisionReason::InvalidSandboxProfile);
    }
    if !target_sandbox.allows(capability) {
        return PolicyDecision::deny(DecisionReason::SandboxCeiling);
    }
    PolicyDecision::allow()
}

/// Only a controller may bind/rebind a sandbox profile. A profile is always
/// subordinate to the Stage 9.1 security-domain ceiling; this prevents a
/// sandbox change from becoming a privilege-escalation path.
pub fn authorize_sandbox_bind(
    actor_domain: SecurityDomain,
    actor_capabilities: CapabilitySet,
    target_domain: SecurityDomain,
    profile: SandboxProfile,
) -> PolicyDecision {
    if !actor_capabilities.contains(Capability::TaskControl) {
        return PolicyDecision::deny(DecisionReason::ActorLacksTaskControl);
    }
    if target_domain == SecurityDomain::Kernel && actor_domain != SecurityDomain::Kernel {
        return PolicyDecision::deny(DecisionReason::KernelDomainProtected);
    }
    if !profile.is_valid_for(target_domain) {
        return PolicyDecision::deny(DecisionReason::InvalidSandboxProfile);
    }
    PolicyDecision::allow()
}


/// Stage 9.4B authorization for service-registry publication.
pub fn authorize_service_publish(profile: SandboxProfile, class: ServiceClass) -> PolicyDecision {
    if profile.allows_service_publish(class) {
        PolicyDecision::allow()
    } else {
        PolicyDecision::deny(DecisionReason::ServicePublishDenied)
    }
}

/// Stage 9.4B authorization for service discovery and subsequent use of a
/// service-derived handle. Rechecking on use makes profile tightening immediate.
pub fn authorize_service_discover(profile: SandboxProfile, class: ServiceClass) -> PolicyDecision {
    if profile.allows_service_discover(class) {
        PolicyDecision::allow()
    } else {
        PolicyDecision::deny(DecisionReason::ServiceDiscoverDenied)
    }
}

/// Revocation removes authority and therefore cannot amplify privileges. It
/// still requires TaskControl so unprivileged tasks cannot mutate another
/// task's security state.
pub fn authorize_revoke(actor_capabilities: CapabilitySet) -> PolicyDecision {
    if actor_capabilities.contains(Capability::TaskControl) {
        PolicyDecision::allow()
    } else {
        PolicyDecision::deny(DecisionReason::ActorLacksTaskControl)
    }
}

/// Allocation-free Stage 9.1 policy proof used by the production boot path.
pub fn self_test() -> bool {
    let kernel = CapabilitySet::kernel_bootstrap();
    let user = CapabilitySet::userspace();

    authorize_grant(
        SecurityDomain::Kernel,
        kernel,
        SecurityDomain::Restricted,
        SandboxProfile::RESTRICTED,
        Capability::TimerRead,
    )
    .allowed
        && !authorize_grant(
            SecurityDomain::Kernel,
            kernel,
            SecurityDomain::Restricted,
            SandboxProfile::RESTRICTED,
            Capability::DeviceIo,
        )
        .allowed
        && authorize_grant(
            SecurityDomain::Kernel,
            kernel,
            SecurityDomain::User,
            SandboxProfile::USER_DEFAULT,
            Capability::Ipc,
        )
        .allowed
        && !authorize_grant(
            SecurityDomain::Kernel,
            kernel,
            SecurityDomain::User,
            SandboxProfile::USER_DEFAULT,
            Capability::MemoryInspect,
        )
        .allowed
        && !authorize_grant(
            SecurityDomain::User,
            user,
            SecurityDomain::User,
            SandboxProfile::USER_DEFAULT,
            Capability::Ipc,
        )
        .allowed
        && !authorize_grant(
            SecurityDomain::Kernel,
            kernel,
            SecurityDomain::User,
            SandboxProfile::new(0x94a0, CapabilitySet::only(Capability::Console)),
            Capability::Ipc,
        )
        .allowed
        && !authorize_sandbox_bind(
            SecurityDomain::Kernel,
            kernel,
            SecurityDomain::Restricted,
            SandboxProfile::USER_DEFAULT,
        )
        .allowed
        && authorize_service_publish(
            SandboxProfile::RESTRICTED.with_service_policy(ServicePolicy::only(
                ServiceClass::Core,
                ServiceClass::Network,
            )),
            ServiceClass::Core,
        )
        .allowed
        && !authorize_service_discover(
            SandboxProfile::RESTRICTED.with_service_policy(ServicePolicy::only(
                ServiceClass::Core,
                ServiceClass::Network,
            )),
            ServiceClass::Ai,
        )
        .allowed
        && authorize_device_access(
            user,
            SandboxProfile::USER_DEFAULT,
            DeviceClass::Network,
        ).allowed
        && !authorize_device_access(
            user,
            SandboxProfile::USER_DEFAULT,
            DeviceClass::Storage,
        ).allowed
        && !authorize_device_access(
            kernel,
            SandboxProfile::KERNEL_TRUSTED.with_device_policy(DevicePolicy::only(DeviceClass::Network)),
            DeviceClass::Storage,
        ).allowed
        && authorize_revoke(kernel).allowed
        && !authorize_revoke(user).allowed
}

// -------------------------------------------------------------------------
// Stage 9.2A: bounded generation-tagged capability lineage foundation.
// -------------------------------------------------------------------------

/// Bounded lineage storage. 64 entries keeps the proof allocation-free and
/// makes exhaustion deterministic rather than turning security metadata into
/// an unbounded heap consumer.
pub const MAX_CAPABILITY_LINEAGES: usize = 64;
const LINEAGE_SLOT_BITS: u32 = 8;
const LINEAGE_SLOT_MASK: u32 = (1 << LINEAGE_SLOT_BITS) - 1;
const LINEAGE_GENERATION_MASK: u32 = (1 << (32 - LINEAGE_SLOT_BITS)) - 1;

/// Generation-tagged identifier for a lineage node.
///
/// The low byte stores slot+1 (zero is always invalid); the upper 24 bits store
/// a generation. Reusing a slot therefore cannot silently resurrect a stale
/// token from an earlier capability lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineageId(u32);

impl LineageId {
    const fn from_parts(slot: usize, generation: u32) -> Self {
        Self((generation << LINEAGE_SLOT_BITS) | (slot as u32 + 1))
    }

    const fn slot(self) -> Option<usize> {
        let encoded = self.0 & LINEAGE_SLOT_MASK;
        if encoded == 0 {
            None
        } else {
            Some((encoded - 1) as usize)
        }
    }

    const fn generation(self) -> u32 {
        (self.0 >> LINEAGE_SLOT_BITS) & LINEAGE_GENERATION_MASK
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineageError {
    EmptyRights,
    RightsAmplification,
    InvalidLineage,
    PermissionDenied,
    TableFull,
    HasChildren,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineageInfo {
    pub id: LineageId,
    pub parent: Option<LineageId>,
    pub owner: u64,
    pub rights: CapabilitySet,
    pub child_count: u16,
}

#[derive(Clone, Copy)]
struct LineageSlot {
    generation: u32,
    active: bool,
    parent: Option<LineageId>,
    owner: u64,
    rights: CapabilitySet,
    child_count: u16,
}

impl LineageSlot {
    const fn empty() -> Self {
        Self {
            generation: 1,
            active: false,
            parent: None,
            owner: 0,
            rights: CapabilitySet::empty(),
            child_count: 0,
        }
    }

    fn id(&self, slot: usize) -> LineageId {
        LineageId::from_parts(slot, self.generation)
    }

    fn clear_for_reuse(&mut self) {
        self.active = false;
        self.parent = None;
        self.owner = 0;
        self.rights = CapabilitySet::empty();
        self.child_count = 0;
        self.generation = next_generation(self.generation);
    }
}

const fn next_generation(current: u32) -> u32 {
    let next = (current + 1) & LINEAGE_GENERATION_MASK;
    if next == 0 { 1 } else { next }
}

struct LineageRegistry {
    slots: [LineageSlot; MAX_CAPABILITY_LINEAGES],
    count: usize,
}

impl LineageRegistry {
    const fn new() -> Self {
        Self {
            slots: [const { LineageSlot::empty() }; MAX_CAPABILITY_LINEAGES],
            count: 0,
        }
    }

    fn lookup_index(&self, id: LineageId) -> Option<usize> {
        let slot = id.slot()?;
        if slot >= MAX_CAPABILITY_LINEAGES {
            return None;
        }
        let entry = &self.slots[slot];
        (entry.active && entry.generation == id.generation()).then_some(slot)
    }

    fn allocate(
        &mut self,
        parent: Option<LineageId>,
        owner: u64,
        rights: CapabilitySet,
    ) -> Result<LineageId, LineageError> {
        if rights.is_empty() {
            return Err(LineageError::EmptyRights);
        }
        let Some(index) = self.slots.iter().position(|slot| !slot.active) else {
            return Err(LineageError::TableFull);
        };
        let entry = &mut self.slots[index];
        entry.active = true;
        entry.parent = parent;
        entry.owner = owner;
        entry.rights = rights;
        entry.child_count = 0;
        self.count += 1;
        Ok(entry.id(index))
    }

    fn issue_root(&mut self, owner: u64, rights: CapabilitySet) -> Result<LineageId, LineageError> {
        self.allocate(None, owner, rights)
    }

    fn derive(
        &mut self,
        actor: u64,
        parent: LineageId,
        new_owner: u64,
        rights: CapabilitySet,
    ) -> Result<LineageId, LineageError> {
        if rights.is_empty() {
            return Err(LineageError::EmptyRights);
        }
        let Some(parent_index) = self.lookup_index(parent) else {
            return Err(LineageError::InvalidLineage);
        };
        let parent_entry = self.slots[parent_index];
        if parent_entry.owner != actor {
            return Err(LineageError::PermissionDenied);
        }
        if !rights.is_subset_of(parent_entry.rights) {
            return Err(LineageError::RightsAmplification);
        }
        if parent_entry.child_count == u16::MAX {
            return Err(LineageError::TableFull);
        }

        let child = self.allocate(Some(parent), new_owner, rights)?;
        self.slots[parent_index].child_count += 1;
        Ok(child)
    }

    fn info(&self, id: LineageId) -> Result<LineageInfo, LineageError> {
        let Some(index) = self.lookup_index(id) else {
            return Err(LineageError::InvalidLineage);
        };
        let entry = self.slots[index];
        Ok(LineageInfo {
            id,
            parent: entry.parent,
            owner: entry.owner,
            rights: entry.rights,
            child_count: entry.child_count,
        })
    }

    fn is_descendant_of(&self, descendant: LineageId, ancestor: LineageId) -> bool {
        let mut current = Some(descendant);
        for _ in 0..MAX_CAPABILITY_LINEAGES {
            let Some(id) = current else {
                return false;
            };
            if id == ancestor {
                return descendant != ancestor;
            }
            let Ok(info) = self.info(id) else {
                return false;
            };
            current = info.parent;
        }
        false
    }

    fn release(&mut self, actor: u64, id: LineageId) -> Result<(), LineageError> {
        let Some(index) = self.lookup_index(id) else {
            return Err(LineageError::InvalidLineage);
        };
        let entry = self.slots[index];
        if entry.owner != actor {
            return Err(LineageError::PermissionDenied);
        }
        if entry.child_count != 0 {
            return Err(LineageError::HasChildren);
        }

        if let Some(parent) = entry.parent {
            let Some(parent_index) = self.lookup_index(parent) else {
                return Err(LineageError::InvalidLineage);
            };
            if self.slots[parent_index].child_count == 0 {
                return Err(LineageError::InvalidLineage);
            }
            self.slots[parent_index].child_count -= 1;
        }
        self.slots[index].clear_for_reuse();
        self.count -= 1;
        Ok(())
    }

    /// Atomically revoke one lineage node and every live descendant.
    ///
    /// Authorization follows the authority chain: the actor may own the target
    /// itself or any live ancestor of the target. This lets a delegator revoke
    /// authority it issued while preventing unrelated sibling branches from
    /// interfering with one another.
    fn revoke_subtree(&mut self, actor: u64, target: LineageId) -> Result<usize, LineageError> {
        let Some(target_index) = self.lookup_index(target) else {
            return Err(LineageError::InvalidLineage);
        };

        let mut authorized = false;
        let mut current = Some(target);
        for _ in 0..MAX_CAPABILITY_LINEAGES {
            let Some(id) = current else {
                break;
            };
            let Some(index) = self.lookup_index(id) else {
                return Err(LineageError::InvalidLineage);
            };
            let entry = self.slots[index];
            if entry.owner == actor {
                authorized = true;
                break;
            }
            current = entry.parent;
        }
        if !authorized {
            return Err(LineageError::PermissionDenied);
        }

        // Snapshot membership before mutating any generation. This keeps the
        // operation atomic under the registry lock and avoids ancestry walks
        // seeing partially cleared parents.
        let mut revoke = [false; MAX_CAPABILITY_LINEAGES];
        let mut revoked = 0usize;
        for (index, entry) in self.slots.iter().enumerate() {
            if !entry.active {
                continue;
            }
            let id = entry.id(index);
            if id == target || self.is_descendant_of(id, target) {
                revoke[index] = true;
                revoked += 1;
            }
        }
        if revoked == 0 || !revoke[target_index] {
            return Err(LineageError::InvalidLineage);
        }

        // Only the target's parent can survive the subtree operation. Its
        // direct-child count loses exactly the revoked target branch.
        if let Some(parent) = self.slots[target_index].parent {
            let Some(parent_index) = self.lookup_index(parent) else {
                return Err(LineageError::InvalidLineage);
            };
            if !revoke[parent_index] {
                if self.slots[parent_index].child_count == 0 {
                    return Err(LineageError::InvalidLineage);
                }
                self.slots[parent_index].child_count -= 1;
            }
        }

        for (index, should_revoke) in revoke.into_iter().enumerate() {
            if should_revoke {
                self.slots[index].clear_for_reuse();
            }
        }
        self.count -= revoked;
        Ok(revoked)
    }
}

static LINEAGES: Mutex<LineageRegistry> = Mutex::new(LineageRegistry::new());

/// Execute one lineage-registry operation with local interrupts disabled.
///
/// Stage 9.2C made lineage lookup/revocation reachable from scheduler lifecycle
/// paths, including dead-task reaping. Those paths can run from a timer-driven
/// scheduling interrupt. A plain spin::Mutex is not re-entrant: if an interrupt
/// preempts this CPU while it owns LINEAGES and the interrupt path tries to lock
/// LINEAGES again, the CPU spins forever waiting on itself. Disabling local
/// interrupts for the complete lock lifetime removes that same-CPU re-entrancy
/// hazard while preserving normal SMP exclusion against other CPUs.
fn with_lineages<R>(operation: impl FnOnce(&mut LineageRegistry) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut registry = LINEAGES.lock();
        operation(&mut registry)
    })
}

/// Create a lineage root for already-authorized authority.
///
/// This remains kernel-internal security metadata. Stage 9.2B adds recursive
/// invalidation to the lineage primitive; binding all task capabilities and IPC
/// handles to lineage identities is intentionally deferred to a later step.
pub fn issue_lineage_root(owner: u64, rights: CapabilitySet) -> Result<LineageId, LineageError> {
    let result = with_lineages(|registry| registry.issue_root(owner, rights));
    crate::audit::record_detail(
        owner,
        crate::audit::Action::CapabilityLineageIssue,
        result.map_or(0, LineageId::as_u32) as u64,
        rights.bits(),
        result.is_ok(),
    );
    result
}

/// Derive a child token. Rights may only stay equal or decrease.
pub fn derive_lineage(
    actor: u64,
    parent: LineageId,
    new_owner: u64,
    rights: CapabilitySet,
) -> Result<LineageId, LineageError> {
    let result = with_lineages(|registry| registry.derive(actor, parent, new_owner, rights));
    crate::audit::record_detail(
        actor,
        crate::audit::Action::CapabilityDerived,
        result.map_or(parent.as_u32(), LineageId::as_u32) as u64,
        rights.bits(),
        result.is_ok(),
    );
    result
}

pub fn lineage_info(id: LineageId) -> Result<LineageInfo, LineageError> {
    with_lineages(|registry| registry.info(id))
}

/// Return whether a live lineage token still authorizes one capability.
///
/// Stage 9.2C uses this at task capability check time. A revoked/stale token
/// therefore stops authorizing the bound capability immediately without a
/// second mutable sweep through scheduler state.
pub fn lineage_authorizes(id: LineageId, capability: Capability) -> bool {
    with_lineages(|registry| {
        registry
            .info(id)
            .is_ok_and(|info| info.rights.contains(capability))
    })
}

pub fn lineage_is_descendant_of(descendant: LineageId, ancestor: LineageId) -> bool {
    with_lineages(|registry| registry.is_descendant_of(descendant, ancestor))
}

/// Release one leaf token. This is lifecycle cleanup, not revocation: parents
/// with live descendants are intentionally protected until Stage 9.2B adds
/// explicit recursive invalidation semantics.
pub fn release_lineage(actor: u64, id: LineageId) -> Result<(), LineageError> {
    let result = with_lineages(|registry| registry.release(actor, id));
    crate::audit::record(
        actor,
        crate::audit::Action::CapabilityLineageRelease,
        id.as_u32() as u64,
        result.is_ok(),
    );
    result
}

pub fn lineage_count() -> usize {
    with_lineages(|registry| registry.count)
}

/// Lifecycle cleanup for a retiring task. Every lineage node directly owned by
/// `owner` is recalled together with its descendants. The bounded loop always
/// re-scans after mutation, so generation changes cannot leave stale indices.
pub fn revoke_all_owned_lineages(owner: u64) -> usize {
    with_lineages(|registry| {
        let mut total = 0usize;
        loop {
            let next = registry
                .slots
                .iter()
                .enumerate()
                .find(|(_, entry)| entry.active && entry.owner == owner)
                .map(|(index, entry)| entry.id(index));
            let Some(id) = next else {
                break;
            };
            match registry.revoke_subtree(owner, id) {
                Ok(revoked) => total = total.saturating_add(revoked),
                Err(_) => break,
            }
        }
        total
    })
}

/// Stage 9.2B recursive revocation. The registry lock covers authorization,
/// subtree discovery, boundary bookkeeping, generation invalidation, and count
/// updates as one transaction. Callers therefore never observe a partially
/// revoked lineage tree.
pub fn revoke_lineage_subtree(actor: u64, target: LineageId) -> Result<usize, LineageError> {
    let result = with_lineages(|registry| registry.revoke_subtree(actor, target));
    crate::audit::record(
        actor,
        crate::audit::Action::CapabilityLineageRevoke,
        target.as_u32() as u64,
        result.is_ok(),
    );
    result
}

/// Revoke a bound lineage after WovenGuard task policy has already authorized
/// the controller. This is intentionally separate from owner/ancestor recall:
/// callers must first hold TaskControl and pass `authorize_revoke`.
pub fn revoke_lineage_subtree_by_controller(
    controller: u64,
    target: LineageId,
) -> Result<usize, LineageError> {
    let result = with_lineages(|registry| {
        let Some(target_index) = registry.lookup_index(target) else {
            return Err(LineageError::InvalidLineage);
        };

        let target_owner = registry.slots[target_index].owner;
        registry.revoke_subtree(target_owner, target)
    });
    crate::audit::record(
        controller,
        crate::audit::Action::CapabilityLineageRevoke,
        target.as_u32() as u64,
        result.is_ok(),
    );
    result
}

/// Production-boot proof for the Stage 9.2A primitive.
///
/// It proves generation-tagged stale-token rejection, two-level ancestry,
/// rights-reducing derivation, anti-amplification, owner checks, parent lifetime
/// protection, deterministic leaf-first cleanup, slot reuse, and zero leakage.
pub fn lineage_self_test() -> bool {
    const ROOT_OWNER: u64 = u64::MAX - 0x920;
    const CHILD_OWNER: u64 = u64::MAX - 0x921;
    const GRANDCHILD_OWNER: u64 = u64::MAX - 0x922;

    let baseline = lineage_count();
    let root_rights = CapabilitySet::empty()
        .with(Capability::Console)
        .with(Capability::Ipc)
        .with(Capability::FileRead);
    let child_rights = CapabilitySet::empty()
        .with(Capability::Console)
        .with(Capability::Ipc);
    let grandchild_rights = CapabilitySet::empty().with(Capability::Ipc);

    let Ok(root) = issue_lineage_root(ROOT_OWNER, root_rights) else {
        return false;
    };
    let Ok(child) = derive_lineage(ROOT_OWNER, root, CHILD_OWNER, child_rights) else {
        let _ = release_lineage(ROOT_OWNER, root);
        return false;
    };
    let Ok(grandchild) = derive_lineage(CHILD_OWNER, child, GRANDCHILD_OWNER, grandchild_rights)
    else {
        let _ = release_lineage(CHILD_OWNER, child);
        let _ = release_lineage(ROOT_OWNER, root);
        return false;
    };

    let root_info_ok = lineage_info(root).is_ok_and(|info| {
        info.id == root
            && info.parent.is_none()
            && info.owner == ROOT_OWNER
            && info.rights == root_rights
            && info.child_count == 1
    });
    let child_info_ok = lineage_info(child).is_ok_and(|info| {
        info.parent == Some(root)
            && info.owner == CHILD_OWNER
            && info.rights == child_rights
            && info.child_count == 1
    });
    let ancestry_ok = lineage_is_descendant_of(child, root)
        && lineage_is_descendant_of(grandchild, root)
        && lineage_is_descendant_of(grandchild, child)
        && !lineage_is_descendant_of(root, root)
        && !lineage_is_descendant_of(root, child);
    let escalation_denied = derive_lineage(
        CHILD_OWNER,
        child,
        GRANDCHILD_OWNER,
        CapabilitySet::empty()
            .with(Capability::Ipc)
            .with(Capability::FileWrite),
    ) == Err(LineageError::RightsAmplification);
    let wrong_owner_denied = derive_lineage(ROOT_OWNER, child, ROOT_OWNER, grandchild_rights)
        == Err(LineageError::PermissionDenied);
    let parent_release_blocked =
        release_lineage(ROOT_OWNER, root) == Err(LineageError::HasChildren);

    let cleanup_ok = release_lineage(GRANDCHILD_OWNER, grandchild).is_ok()
        && release_lineage(CHILD_OWNER, child).is_ok()
        && release_lineage(ROOT_OWNER, root).is_ok();
    let stale_rejected = lineage_info(root) == Err(LineageError::InvalidLineage);

    let generation_ok = if cleanup_ok {
        match issue_lineage_root(ROOT_OWNER, root_rights) {
            Ok(reused) => {
                let changed = reused != root;
                let released = release_lineage(ROOT_OWNER, reused).is_ok();
                changed && released
            }
            Err(_) => false,
        }
    } else {
        false
    };

    let bounded_capacity_ok = {
        let mut registry = LineageRegistry::new();
        let mut ids = [None; MAX_CAPABILITY_LINEAGES];
        let mut filled = true;
        for (index, slot) in ids.iter_mut().enumerate() {
            match registry.issue_root(index as u64 + 1, grandchild_rights) {
                Ok(id) => *slot = Some(id),
                Err(_) => {
                    filled = false;
                    break;
                }
            }
        }
        let full_rejected = registry.issue_root(u64::MAX, grandchild_rights)
            == Err(LineageError::TableFull);
        let mut cleaned = true;
        for (index, id) in ids.into_iter().enumerate() {
            if let Some(id) = id {
                cleaned &= registry.release(index as u64 + 1, id).is_ok();
            }
        }
        filled && full_rejected && cleaned && registry.count == 0
    };

    root_info_ok
        && child_info_ok
        && ancestry_ok
        && escalation_denied
        && wrong_owner_denied
        && parent_release_blocked
        && cleanup_ok
        && stale_rejected
        && generation_ok
        && bounded_capacity_ok
        && lineage_count() == baseline
}


/// Production-boot proof for Stage 9.2B recursive capability revocation.
///
/// The topology intentionally contains two sibling branches. Revoking one
/// branch must invalidate its descendants generation-safely while the sibling
/// branch remains usable. The root issuer may revoke delegated descendants; an
/// unrelated sibling owner may not.
pub fn revocation_self_test() -> bool {
    const ROOT_OWNER: u64 = u64::MAX - 0x92b0;
    const BRANCH_OWNER: u64 = u64::MAX - 0x92b1;
    const SIBLING_OWNER: u64 = u64::MAX - 0x92b2;
    const LEAF_OWNER: u64 = u64::MAX - 0x92b3;

    let baseline = lineage_count();
    let root_rights = CapabilitySet::empty()
        .with(Capability::Console)
        .with(Capability::Ipc)
        .with(Capability::FileRead);
    let branch_rights = CapabilitySet::empty()
        .with(Capability::Ipc)
        .with(Capability::FileRead);
    let leaf_rights = CapabilitySet::empty().with(Capability::Ipc);

    let Ok(root) = issue_lineage_root(ROOT_OWNER, root_rights) else {
        return false;
    };
    let Ok(branch) = derive_lineage(ROOT_OWNER, root, BRANCH_OWNER, branch_rights) else {
        let _ = release_lineage(ROOT_OWNER, root);
        return false;
    };
    let Ok(sibling) = derive_lineage(ROOT_OWNER, root, SIBLING_OWNER, leaf_rights) else {
        let _ = revoke_lineage_subtree(ROOT_OWNER, root);
        return false;
    };
    let Ok(leaf) = derive_lineage(BRANCH_OWNER, branch, LEAF_OWNER, leaf_rights) else {
        let _ = revoke_lineage_subtree(ROOT_OWNER, root);
        return false;
    };
    let Ok(deep_leaf) = derive_lineage(LEAF_OWNER, leaf, LEAF_OWNER + 1, leaf_rights) else {
        let _ = revoke_lineage_subtree(ROOT_OWNER, root);
        return false;
    };

    // A sibling branch has no authority over its peer.
    let sibling_denied = revoke_lineage_subtree(SIBLING_OWNER, branch)
        == Err(LineageError::PermissionDenied);

    // The root issuer controls descendants of authority it delegated.
    let branch_revoked = revoke_lineage_subtree(ROOT_OWNER, branch) == Ok(3);
    let stale_branch = lineage_info(branch) == Err(LineageError::InvalidLineage);
    let stale_leaf = lineage_info(leaf) == Err(LineageError::InvalidLineage);
    let stale_deep_leaf = lineage_info(deep_leaf) == Err(LineageError::InvalidLineage);
    let derive_from_revoked = derive_lineage(BRANCH_OWNER, branch, LEAF_OWNER, leaf_rights)
        == Err(LineageError::InvalidLineage);

    // The root and sibling survive, with the root's direct-child count reduced
    // from two branches to exactly one. The surviving sibling remains usable.
    let root_boundary_ok = lineage_info(root).is_ok_and(|info| info.child_count == 1);
    let sibling_survives = lineage_info(sibling).is_ok_and(|info| {
        info.parent == Some(root) && info.owner == SIBLING_OWNER && info.rights == leaf_rights
    });
    let sibling_can_derive = match derive_lineage(SIBLING_OWNER, sibling, LEAF_OWNER, leaf_rights) {
        Ok(child) => revoke_lineage_subtree(SIBLING_OWNER, child) == Ok(1),
        Err(_) => false,
    };

    // Revoking the root removes the remaining branch and the root itself.
    let root_revoked = revoke_lineage_subtree(ROOT_OWNER, root) == Ok(2);
    let root_stale = lineage_info(root) == Err(LineageError::InvalidLineage);
    let sibling_stale = lineage_info(sibling) == Err(LineageError::InvalidLineage);

    // A reused slot must receive a different generation from a revoked token.
    let generation_after_revoke_ok = match issue_lineage_root(ROOT_OWNER, leaf_rights) {
        Ok(reused) => {
            let changed = reused != root && reused != branch && reused != leaf && reused != deep_leaf;
            let cleanup = revoke_lineage_subtree(ROOT_OWNER, reused) == Ok(1);
            changed && cleanup
        }
        Err(_) => false,
    };

    sibling_denied
        && branch_revoked
        && stale_branch
        && stale_leaf
        && stale_deep_leaf
        && derive_from_revoked
        && root_boundary_ok
        && sibling_survives
        && sibling_can_derive
        && root_revoked
        && root_stale
        && sibling_stale
        && generation_after_revoke_ok
        && lineage_count() == baseline
}
