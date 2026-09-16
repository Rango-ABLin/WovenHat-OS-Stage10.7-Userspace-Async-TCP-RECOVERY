//! Central compile-time configuration and resource limits for WovenHat.
//!
//! Raising these values is the first step of Phase A (scalability).
//! Keep them power-of-two friendly where it helps alignment, but the
//! primary goal is to remove the artificial 8-slot ceilings that
//! currently prevent real multi-process workloads.

/// Maximum number of tasks the scheduler can track (including kernel + idle).
pub const MAX_TASKS: usize = 32;

/// Maximum number of processes (user + kernel).
pub const MAX_PROCESSES: usize = 32;

/// Maximum open file descriptors per process.
/// Descriptors 0, 1, 2 are reserved for stdin / stdout / stderr.
pub const MAX_FILE_DESCRIPTORS: usize = 16;

/// Kernel stack size reserved for each task (bytes).
// Debug fork copies bounded process metadata and walks sparse mappings.
// Reserve room for nested calls and fault population on the same entry stack.
pub const TASK_STACK_SIZE: usize = 4096 * 32;

/// Maximum IPC endpoints (one per process is typical).
pub const MAX_IPC_ENDPOINTS: usize = 32;

/// Stage 8.1 global kernel IPC object-table capacity. Endpoint objects are
/// addressed internally by opaque object IDs rather than process IDs.
pub const MAX_IPC_OBJECTS: usize = 64;

/// Stage 8.1 process-local IPC handle capacity. Handles are generation-tagged
/// so a stale closed handle cannot alias a later object in the same slot.
pub const MAX_IPC_HANDLES_PER_PROCESS: usize = 16;

/// Stage 8.6 bounded global service-registry capacity. Each published service
/// pins one endpoint object until it is explicitly unpublished or its owner
/// leaves the IPC namespace.
pub const MAX_IPC_SERVICES: usize = 32;

/// Maximum UTF-8/ASCII service-name length accepted by the Stage 8.6 registry.
/// Service names are intentionally short, allocation-free kernel identifiers.
pub const MAX_IPC_SERVICE_NAME: usize = 32;

/// Stage 8.4 bounded shared-memory object capacity. Shared-memory objects use
/// the same process-local handle tables as endpoint objects.
pub const MAX_SHARED_MEMORY_OBJECTS: usize = 32;

/// Maximum number of 4 KiB pages in one Stage 8.4 shared-memory object.
/// The initial foundation deliberately stays bounded and allocation-free.
pub const MAX_SHARED_MEMORY_PAGES: usize = 16;

/// Maximum simultaneously registered mappings per shared-memory object.
pub const MAX_SHARED_MEMORY_MAPPINGS: usize = 32;

/// Maximum messages queued on a single IPC endpoint.
pub const IPC_QUEUE_DEPTH: usize = 16;

/// Maximum payload size of an IPC message (bytes).
pub const MAX_MESSAGE_SIZE: usize = 256;

/// Maximum path length accepted by VFS / syscalls (bytes).
pub const MAX_PATH_SIZE: usize = 128;

/// Maximum size of a single read/write syscall buffer (bytes).
pub const MAX_IO_SIZE: usize = 1024;

/// Maximum VFS nodes (files) that can exist simultaneously.
pub const MAX_VFS_NODES: usize = 64;

/// Capacity of each VFS node data buffer (bytes).
pub const VFS_NODE_CAPACITY: usize = 64 * 1024;

/// Maximum ELF loadable segments accepted by the loader.
pub const MAX_ELF_SEGMENTS: usize = 8;

/// Maximum anonymous (mmap) mappings per process.
pub const MAX_ANONYMOUS_MAPPINGS: usize = 16;

/// Maximum devices registered in the device table.
pub const MAX_DEVICES: usize = 32;

/// Maximum allocations tracked by the simple kernel heap.
pub const MAX_HEAP_ALLOCATIONS: usize = 512;

/// Global open-file description table capacity (refcount-shared across processes).
pub const MAX_OPEN_FILES: usize = 64;

/// Outstanding lazy file-page faults owned by the bounded pager worker.
/// A full queue is treated as an unresolvable user fault; it never grows the
/// kernel heap while handling an exception.
pub const MAX_PAGER_REQUESTS: usize = 16;

/// Outstanding asynchronous block-sector requests owned by the storage worker.
pub const MAX_BLOCK_IO_REQUESTS: usize = 16;

/// Stage 10.2 global generation-tagged asynchronous operation table.
/// The table is allocation-free and shared by storage, networking, device,
/// and service producers as those subsystems migrate onto the common API.
pub const MAX_ASYNC_OPERATIONS: usize = 32;
/// Bounded Stage 10.5 asynchronous file request table.
pub const MAX_ASYNC_FILE_REQUESTS: usize = 16;
pub const MAX_ASYNC_NETWORK_REQUESTS: usize = 16;

/// Swap slots for evicted private mapped pages.
/// Slots prefer configured disk backing and fall back to kernel RAM.
pub const MAX_SWAP_SLOTS: usize = 32;
