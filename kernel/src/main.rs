#![cfg_attr(feature = "qemu-test", allow(dead_code, unused_imports))]
#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

extern crate alloc;

mod ata;
mod async_op;
mod async_file;
mod async_network;
mod async_events;
mod deadline;
#[allow(dead_code)]
mod thread;
mod notifications;
#[cfg(feature = "stage12-1-test")]
mod vfs_api;
#[cfg(any(feature = "stage12-2-test", feature = "stage12-3-test", feature = "stage12-4-test", feature = "stage12-5-test"))]
mod wovenfs;
#[cfg(feature = "stage12-3-test")]
mod volume_crypto;
#[cfg(feature = "stage12-4-test")]
mod snapshots;
#[cfg(feature = "stage12-5-test")]
mod storage_manager;
#[cfg(feature = "stage10-8-test")]
mod async_acceptance;
mod audit;
mod benchmark;
mod block;
mod block_cache;
mod block_io;
mod capability;
mod config;
mod console;
mod completion_queue;
mod completion_port;
mod device;
#[cfg(any(feature = "stage13-1-test", feature = "stage13-2-test", feature = "stage13-3-test", feature = "stage13-4-test", feature = "stage13-5-test", feature = "stage13-6-test", feature = "stage13-7-test", feature = "stage13-8-test", feature = "stage13-9-test"))]
mod driver;
mod journal;
mod elf;
mod entropy;
mod fat32;
mod file_frames;
mod file_mapping;
mod gdt;
mod gpt;
mod graphics;
mod gui;
mod hal;
mod heap;
mod interrupts;
mod ipc;
mod irq_lock;
mod keyboard;
mod woven_input;
#[cfg(feature = "stage13-8-test")]
mod hda;
#[cfg(feature = "stage13-8-test")]
mod woven_audio;
#[cfg(feature = "stage13-9-test")]
mod wifi;
#[cfg(feature = "stage13-9-test")]
mod wifi80211;
#[cfg(feature = "stage13-9-test")]
mod wifi_scan;
#[cfg(feature = "stage13-9-test")]
mod wifi_link;
#[cfg(feature = "stage13-9-test")]
mod wifi_rsn;
#[cfg(feature = "stage13-9-test")]
mod wifi_crypto;
#[cfg(feature = "stage13-9-test")]
mod wifi_wpa2;
#[cfg(feature = "stage13-9-test")]
mod wifi_gtk;
#[cfg(feature = "stage13-9-test")]
mod wifi_ccmp;
#[cfg(feature = "stage13-9-test")]
mod wifi_net;
#[cfg(feature = "stage13-9-test")]
mod wifi_backend;
#[cfg(feature = "stage13-9-test")]
mod wifi_hw;
#[cfg(feature = "stage13-9-test")]
mod wifi_recovery;
#[cfg(feature = "stage13-9-test")]
mod wifi_session;
mod wifi_smol;
#[cfg(feature = "stage13-9-test")]
mod wifi_security;
mod memory;
mod network;
#[cfg(feature = "stage13-3-test")]
mod nvme;
#[cfg(feature = "stage13-4-test")]
mod ahci;
#[cfg(any(feature = "stage13-5-test", feature = "stage13-6-test", feature = "stage13-7-test", feature = "stage13-8-test", feature = "stage13-9-test"))]
mod xhci;
mod page_cache;
mod paging;
mod panic;
mod partition;
mod pic;
mod pipe;
mod serial;
mod shell;
mod storage;
mod swap;
mod smp;
mod syscall;
mod task;
mod terminal;
mod timer;
mod userspace;
mod vfs;
mod virtio_net;
mod wovenguard;

use bootloader_api::{config::Mapping, entry_point, info::Optional, BootInfo, BootloaderConfig};

use console::Console;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::{alloc::Layout, panic::PanicInfo};
use shell::Shell;

use x86_64::instructions::interrupts::int3;

static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    // The default boot stack is intentionally small. WovenHat performs
    // substantial early initialization, so give the bootstrap task a 1 MiB
    // stack while retaining the bootloader's guard-page protection.
    config.kernel_stack_size = 1024 * 1024;
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

static PREEMPTION_PROBE_BLOCKED: AtomicBool = AtomicBool::new(false);
static PREEMPTION_PROBE_COMPLETED: AtomicBool = AtomicBool::new(false);
static FAIR_TASK_A_RUNS: AtomicU64 = AtomicU64::new(0);
static FAIR_TASK_B_RUNS: AtomicU64 = AtomicU64::new(0);
static FAIR_TASKS_COMPLETED: AtomicU64 = AtomicU64::new(0);

#[allow(unreachable_code)]
fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    smp::prepare(&boot_info.memory_regions);
    let physical_memory_offset = match &boot_info.physical_memory_offset {
        Optional::Some(offset) => Some(*offset),
        Optional::None => None,
    };
    let paging_init = match physical_memory_offset {
        Some(offset) => paging::init(offset),
        None => Err(paging::InitError::MissingPhysicalMemoryMapping),
    };
    let rsdp_address = match &boot_info.rsdp_addr {
        Optional::Some(address) => Some(*address),
        Optional::None => None,
    };
    let acpi = physical_memory_offset
        .ok_or(hal::acpi::Error::OutOfRange)
        .and_then(|offset| hal::acpi::discover(offset, rsdp_address, &boot_info.memory_regions));
    let memory_init = match (acpi.as_ref(), physical_memory_offset) {
        (Ok(summary), Some(offset)) => memory::init_with_topology(
            &boot_info.memory_regions,
            &summary.memory_affinities[..summary.memory_affinity_count],
            offset,
        ),
        (_, Some(offset)) => memory::init(&boot_info.memory_regions, offset),
        (_, None) => Err(memory::InitError::MissingPhysicalMemoryMapping),
    };
    let boot_info_address = boot_info as *const BootInfo as u64;

    let framebuffer = match &mut boot_info.framebuffer {
        Optional::Some(framebuffer) => framebuffer,
        Optional::None => halt(),
    };

    let info = framebuffer.info();
    let buffer = framebuffer.buffer_mut();
    let framebuffer_address = buffer.as_ptr() as u64;
    let stack_probe = &info as *const _ as u64;

    terminal::init(buffer, info);
    let mut console = Console::new(buffer, info, 40, 40, 2);

    console.clear();

    console.println("WOVENHAT OS");
    console.println("SECURE INTELLIGENCE PLATFORM");
    console.println("");

    console.println("WOVENHAT KERNEL 0.8.0 MULTICORE FOUNDATION");
    console.println("ARCHITECTURE: X86_64");
    console.println("KERNEL BOOT SUCCESSFUL.");
    console.println("");

    if memory_init.is_err() {
        console.println("FRAME ALLOCATOR: INITIALIZATION FAILED");
        halt();
    }

    if memory::self_test() {
        console.println("FRAME ALLOCATOR: OK");
        let memory_stats = memory::stats();
        serial::write_line(format_args!(
            "[MEMORY] frame allocator NUMA domains={} regions={}",
            memory_stats.numa_domains,
            memory_stats.usable_regions
        ));
    } else {
        console.println("FRAME ALLOCATOR: SELF TEST FAILED");
        halt();
    }

    if paging_init.is_err() {
        console.println("PAGING: INITIALIZATION FAILED");
        halt();
    }

    let translation_probes = [
        kernel_main as *const () as u64,
        boot_info_address,
        framebuffer_address,
        stack_probe,
    ];
    if paging::self_test(&translation_probes) {
        console.println("PAGING TRANSLATION: 4/4 OK");
    } else {
        console.println("PAGING TRANSLATION: FAILED");
        halt();
    }

    if paging::mapping_self_test() {
        console.println("PAGING MAP/WRITE/UNMAP: OK");
    } else {
        console.println("PAGING MAP/WRITE/UNMAP: FAILED");
        halt();
    }
    if !paging::map_range_rollback_self_test() {
        console.println("PAGING MAP ROLLBACK: FAILED");
        halt();
    }

    serial::init();
    #[cfg(feature = "qemu-test")]
    if !paging::table_allocation_rollback_self_test() {
        serial::write_line(format_args!("[S6.PAGING] table allocation rollback: FAILED"));
        qemu_test_exit_failure();
    }
    #[cfg(feature = "qemu-test")]
    serial::write_line(format_args!("[S6.PAGING] table allocation rollback: PASSED"));

    let hardware = hal::init(acpi.as_ref().ok());
    if !hal::acpi::self_test() {
        console.println("ACPI PARSER: VALIDATION FAILED");
        halt();
    }
    match acpi {
        Ok(summary) => {
            console.println("ACPI TABLES: VALIDATED");
            serial::write_line(format_args!(
                "[ACPI] revision={} tables={} APIC={} CPUs={} IOAPICs={} ISOs={} LAPIC={:#x} FADT={} HPET={} MCFG={} SRAT_MEM={} NUMA_DOMAINS={} truncated={}",
                summary.revision,
                summary.tables,
                summary.apic as u8,
                summary.enabled_processors,
                summary.io_apics,
                summary.interrupt_overrides,
                summary.local_apic_address,
                summary.fadt as u8,
                summary.hpet as u8,
                summary.mcfg as u8,
                summary.memory_affinity_count,
                summary.numa_domains,
                summary.truncated as u8,
            ));
        }
        Err(_) => console.println("ACPI TABLES: UNAVAILABLE"),
    }
    let vendor = match hardware.cpu_vendor {
        hal::CpuVendor::Intel => "INTEL",
        hal::CpuVendor::Amd => "AMD",
        hal::CpuVendor::Unknown => "UNKNOWN",
    };
    serial::write_fmt(format_args!(
        "HARDWARE: CPU={} LOGICAL_CPUS={} TSC={} RDRAND={} AES_NI={} AVX={} PAE={} SSE4.2={}\n",
        vendor,
        hardware.logical_cpus,
        hardware.cpu_features.has_tsc as u8,
        hardware.cpu_features.has_rdrand as u8,
        hardware.cpu_features.has_aes_ni as u8,
        hardware.cpu_features.has_avx as u8,
        hardware.cpu_features.has_pae as u8,
        hardware.cpu_features.has_sse4_2 as u8,
    ));
    if !hal::pci::self_test() {
        console.println("PCI DISCOVERY: VALIDATION FAILED");
        halt();
    }
    serial::write_line(format_args!(
        "[PCI] devices={} recorded={} storage={} network={} display={} bridges={} truncated={}",
        hardware.pci.discovered,
        hardware.pci.recorded,
        hardware.pci.storage,
        hardware.pci.network,
        hardware.pci.display,
        hardware.pci.bridges,
        hardware.pci.truncated as u8,
    ));
    console.println("PCI CONFIGURATION: ENUMERATED");
    #[cfg(feature = "stage13-9-test")]
    {
        let wifi_pci = wifi::discover_pci();
        if !wifi::self_test() {
            serial::write_line(format_args!(
                "[S13.9A] WovenWiFi framework + PCI classification: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9A] WovenWiFi framework + PCI classification: PASSED network={} wireless_candidates={}",
            wifi_pci.network_controllers,
            wifi_pci.wireless_candidates
        ));
        if !wifi80211::self_test() {
            serial::write_line(format_args!(
                "[S13.9B] IEEE 802.11 frame/IE core: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9B] IEEE 802.11 frame/IE core: PASSED"
        ));
        if !wifi_scan::self_test() {
            serial::write_line(format_args!(
                "[S13.9C] beacon/probe scan pipeline: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9C] beacon/probe scan pipeline: PASSED"
        ));
        if !wifi_link::self_test() {
            serial::write_line(format_args!(
                "[S13.9D] Open System authentication + association: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9D] Open System authentication + association: PASSED"
        ));
        if !wifi_rsn::self_test() {
            serial::write_line(format_args!(
                "[S13.9E] RSN + EAPOL-Key protocol foundation: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9E] RSN + EAPOL-Key protocol foundation: PASSED"
        ));
        if !wifi_crypto::self_test() {
            serial::write_line(format_args!(
                "[S13.9F] WPA2 cryptographic foundation: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9F] WPA2 cryptographic foundation: PASSED"
        ));
        if !wifi_wpa2::self_test() {
            serial::write_line(format_args!(
                "[S13.9G] WPA2 4-way handshake integration: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9G] WPA2 4-way handshake integration: PASSED"
        ));
        if !wifi_gtk::self_test() {
            serial::write_line(format_args!(
                "[S13.9H] GTK + encrypted key data: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9H] GTK + encrypted key data: PASSED"
        ));
        if !wifi_ccmp::self_test() {
            serial::write_line(format_args!(
                "[S13.9I] CCMP protected data path: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9I] CCMP protected data path: PASSED"
        ));
        if !wifi_net::self_test() {
            serial::write_line(format_args!(
                "[S13.9J] WovenWiFi <-> WovenNet integration: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9J] WovenWiFi <-> WovenNet integration: PASSED"
        ));
        if !wifi_net::group_ccmp_self_test() {
            serial::write_line(format_args!("[S13.9X] GTK/group-addressed CCMP data path: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S13.9X] GTK/group-addressed CCMP data path: PASSED"));
        if !wifi_wpa2::lifecycle_self_test() {
            serial::write_line(format_args!("[S13.9Y] WPA2 reconnect/rekey lifecycle: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S13.9Y] WPA2 reconnect/rekey lifecycle: PASSED"));
        if !wifi_wpa2::group_rekey_self_test() {
            serial::write_line(format_args!("[S13.9Z] WPA2 live group-key rekey: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S13.9Z] WPA2 live group-key rekey: PASSED"));
        if !wifi_hw::self_test() {
            serial::write_line(format_args!("[S13.10A] physical PCI Wi-Fi backend boundary: FAILED"));
            qemu_test_exit_failure();
        }
        let hw_candidate = wifi_hw::discover_first();
        serial::write_line(format_args!(
            "[S13.10A] physical PCI Wi-Fi backend boundary: PASSED bound_candidate={}",
            hw_candidate.is_some() as u8
        ));
        if !wifi_hw::stage13_10b_self_test() {
            serial::write_line(format_args!("[S13.10B] Wi-Fi MMIO/DMA/interrupt scaffolding: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S13.10B] Wi-Fi MMIO/DMA/interrupt scaffolding: PASSED"));
        if !wifi_hw::stage13_10c_self_test() {
            serial::write_line(format_args!("[S13.10C] Wi-Fi real MMIO + DMA memory ownership: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S13.10C] Wi-Fi real MMIO + DMA memory ownership: PASSED"));
        if !wifi_hw::stage13_10d_self_test() {
            serial::write_line(format_args!("[S13.10D] Wi-Fi DMA descriptor ownership + queue lifecycle: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S13.10D] Wi-Fi DMA descriptor ownership + queue lifecycle: PASSED"));
        if !wifi_backend::self_test() {
            serial::write_line(format_args!(
                "[S13.9K] Wi-Fi transport/backend contract: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9K] Wi-Fi transport/backend contract: PASSED"
        ));
        if !wifi_recovery::self_test() {
            serial::write_line(format_args!(
                "[S13.9L] reconnect/timeout/lifecycle hardening: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9L] reconnect/timeout/lifecycle hardening: PASSED"
        ));
        if !wifi_session::self_test() {
            serial::write_line(format_args!(
                "[S13.9M] cross-module integration closure: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9M] cross-module integration closure: PASSED"
        ));
        if !wifi_security::self_test() {
            serial::write_line(format_args!(
                "[S13.9N] WPA2 security lifecycle integration: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9N] WPA2 security lifecycle integration: PASSED"
        ));
        if !entropy::self_test() {
            serial::write_line(format_args!(
                "[S13.9O] kernel entropy/secure SNonce boundary: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9O] kernel entropy/secure SNonce boundary: PASSED"
        ));
        if !wifi_wpa2::self_test() {
            serial::write_line(format_args!("[S13.9P] live WPA2 GTK/KRACK integration: FAILED"));
            halt();
        }
        serial::write_line(format_args!("[S13.9P] live WPA2 GTK/KRACK integration: PASSED"));
        if !wifi_link::self_test() {
            serial::write_line(format_args!("[S13.9Q] WPA2 association + RSN integration: FAILED"));
            halt();
        }
        serial::write_line(format_args!("[S13.9Q] WPA2 association + RSN integration: PASSED"));
        if !entropy::self_test() {
            serial::write_line(format_args!("[S13.9R] entropy/network randomness split: FAILED"));
            halt();
        }
        serial::write_line(format_args!("[S13.9R] entropy/network randomness split: PASSED"));
        if !wifi_session::wpa2_handoff_self_test() {
            serial::write_line(format_args!("[S13.9S] WPA2 supplicant/session key handoff: FAILED"));
            halt();
        }
        serial::write_line(format_args!("[S13.9S] WPA2 supplicant/session key handoff: PASSED"));
        if !wifi_session::rx_path_self_test() {
            serial::write_line(format_args!("[S13.9T] backend RX/CCMP/Ethernet integration: FAILED"));
            halt();
        }
        serial::write_line(format_args!("[S13.9T] backend RX/CCMP/Ethernet integration: PASSED"));
        if !wifi_smol::self_test() {
            serial::write_line(format_args!("[S13.9U] WovenWiFi/smoltcp transport adapter: FAILED"));
            halt();
        }
        serial::write_line(format_args!("[S13.9U] WovenWiFi/smoltcp transport adapter: PASSED"));
        if !wifi_smol::transport_selector_self_test() {
            serial::write_line(format_args!("[S13.9V] selectable WovenNet transport integration: FAILED"));
            halt();
        }
        serial::write_line(format_args!("[S13.9V] selectable WovenNet transport integration: PASSED"));
        if !wifi_scan::self_test() {
            serial::write_line(format_args!(
                "[S13.9C] beacon/probe scan pipeline: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S13.9C] beacon/probe scan pipeline: PASSED"
        ));
    }

    if heap::init().is_err() {
        console.println("KERNEL HEAP: INITIALIZATION FAILED");
        halt();
    }

    if heap::self_test() {
        console.println("KERNEL HEAP: CAPACITY/FRAGMENTATION OK");
        serial::write_line(format_args!("[HEAP] mapped={} bytes", heap::stats().size));
    } else {
        console.println("KERNEL HEAP: SELF TEST FAILED");
        halt();
    }
    #[cfg(feature = "qemu-test")]
    if !heap::live_metadata_self_test() {
        serial::write_line(format_args!("[S6.HEAP] unbounded live metadata: FAILED"));
        qemu_test_exit_failure();
    }
    #[cfg(feature = "qemu-test")]
    serial::write_line(format_args!("[S6.HEAP] unbounded live metadata: PASSED"));
    #[cfg(feature = "qemu-test")]
    if !memory::reclaimed_overflow_self_test() {
        serial::write_line(format_args!("[S6.MEMORY] reclaimed overflow/double-free: FAILED"));
        qemu_test_exit_failure();
    }
    #[cfg(feature = "qemu-test")]
    serial::write_line(format_args!("[S6.MEMORY] reclaimed overflow/double-free: PASSED"));

    // Normal boots are shell-first. Do not make the interactive console wait
    // for the exhaustive storage/network/ring3 validation suite. Those tests
    // remain below for `--features qemu-test` builds.
    #[cfg(not(feature = "qemu-test"))]
    {
        gdt::init();
        let _user_segments = gdt::user_segments();
        interrupts::init();
        task::init();

        pic::init();
        timer::init();

        let early_devices = [
            device::Device {
                name: "framebuffer-console",
                kind: device::DeviceKind::Console,
                irq: None,
            },
            device::Device {
                name: "com1",
                kind: device::DeviceKind::Serial,
                irq: None,
            },
            device::Device {
                name: "pit",
                kind: device::DeviceKind::Timer,
                irq: Some(timer::IRQ),
            },
            device::Device {
                name: "ps2-keyboard",
                kind: device::DeviceKind::Keyboard,
                irq: Some(keyboard::IRQ),
            },
        ];

        for dev in early_devices {
            if device::register(dev).is_err() {
                serial::write_line(format_args!("[BOOT] early device registration failed"));
                halt();
            }
        }

        smp::start(acpi.ok(), physical_memory_offset.unwrap());
        if !smp::routed_irq() {
            pic::unmask(timer::IRQ);
            pic::unmask(keyboard::IRQ);
        }
        x86_64::instructions::interrupts::enable();
        if !block_io::start_worker() {
            console.println("BLOCK I/O WORKER: START FAILED");
            halt();
        }
        if !async_file::start_worker() {
            console.println("ASYNC FILE WORKER: START FAILED");
            halt();
        }
        if !async_events::start_worker() { panic!("timer/event worker startup failed"); }
        if !async_network::start_worker() {
            console.println("ASYNC NETWORK WORKER: START FAILED");
            halt();
        }
        if !block_io::async_completion_self_test() {
            console.println("BLOCK I/O COMPLETION: FAILED");
            halt();
        }
        if !task::start_pager() {
            console.println("PAGER: START FAILED");
            halt();
        }

        let ata_sectors = ata::init();
        if let Some(sectors) = ata_sectors {
            serial::write_line(format_args!(
                "[ATA] primary-master online: {} sectors",
                sectors
            ));
        } else {
            serial::write_line(format_args!("[ATA] primary-master not detected"));
        }

        match storage::mount_ata_root() {
            storage::MountStatus::Mounted(count) => {
                let _ = device::register(device::Device {
                    name: "ata0",
                    kind: device::DeviceKind::Block,
                    irq: None,
                });
                serial::write_line(format_args!(
                    "[FS] FAT32 mounted at /mnt; imported {} entries",
                    count
                ));
            }
            storage::MountStatus::NoDevice => {
                serial::write_line(format_args!("[FS] no ATA disk; continuing with RAM VFS"))
            }
            storage::MountStatus::NotFat32 => {
                serial::write_line(format_args!("[FS] ATA disk present but no FAT32 root"))
            }
            storage::MountStatus::Failed => serial::write_line(format_args!(
                "[FS] FAT32 mount failed; continuing with RAM VFS"
            )),
        }

        for dir in ["/etc", "/var", "/home", "/tmp"] {
            let _ = vfs::mkdir(dir);
        }

        match network::init() {
            Ok(()) => serial::write_line(format_args!(
                "[NET] virtio-net + smoltcp online at 10.0.2.15/24"
            )),
            Err(error) => serial::write_line(format_args!(
                "[NET] optional network init skipped: {:?}",
                error
            )),
        }

        serial::write_line(format_args!(
            "[BOOT] shell-first runtime ready; entering diagnostic shell"
        ));

        // Start userspace init -> /bin/sh (non-fatal if spawn fails).
        match userspace::create_init_process() {
            Some(program) => match task::spawn_user_process("init", program) {
                Ok((pid, _)) => {
                    serial::write_line(format_args!(
                        "[BOOT] userspace init/sh scheduled as pid {}",
                        pid.as_u64()
                    ));
                    console.println("USERSPACE INIT+SH: STARTED");
                }
                Err(_) => console.println("USERSPACE INIT+SH: SPAWN FAILED"),
            },
            None => console.println("USERSPACE INIT+SH: IMAGE FAILED"),
        }

        console.println("");
        let mut desktop = gui::Desktop::new(graphics::Color::DARK_BLUE);
        let mut window = gui::Window::new(gui::Rect::new(80, 80, 480, 280), "WOVENHAT DESKTOP");
        window.add_button(gui::Button::new(
            gui::Rect::new(120, 180, 180, 48),
            "ACTIVATE",
            graphics::Color::CYAN,
        ));
        window.add_button(gui::Button::new(
            gui::Rect::new(320, 180, 180, 48),
            "SECOND",
            graphics::Color::CYAN,
        ));
        desktop.add_window(window);
        let mut shell = Shell::new();
        let mut desktop_active = false;

        // Start in the diagnostic shell. F1 toggles to the graphical desktop.
        console.clear();
        console.println("WOVENHAT DIAGNOSTIC SHELL (F1 TO OPEN DESKTOP)");
        shell.print_prompt(&mut console);

        let mut userspace_was_foreground = false;
        loop {
            let userspace_foreground = terminal::foreground_active();
            if userspace_was_foreground && !userspace_foreground {
                console.clear();
                console.println("WOVENHAT DIAGNOSTIC SHELL");
                console.println("USERSPACE SESSION ENDED");
                console.println("");
                shell.print_prompt(&mut console);
                desktop_active = false;
            }
            userspace_was_foreground = userspace_foreground;

            // The kernel UI may consume PS/2 input only when no userspace
            // process owns the foreground terminal.  In particular, do not
            // call keyboard::poll() while /bin/sh is foreground: poll() pops
            // the scancode from the shared queue, which would starve the
            // userspace read(0, ...) syscall and make the shell appear hung.
            if !userspace_foreground {
                if let Some(key) = keyboard::poll() {
                    if matches!(key, keyboard::Key::F1) {
                        desktop_active = !desktop_active;
                        if desktop_active {
                            console.render_desktop(&desktop);
                        } else {
                            console.clear();
                            console.println("WOVENHAT DIAGNOSTIC SHELL (F1 TO OPEN DESKTOP)");
                            shell.print_prompt(&mut console);
                        }
                    } else if desktop_active {
                        let event = match key {
                            keyboard::Key::Char(character) => gui::InputEvent::Key(character),
                            keyboard::Key::Enter => gui::InputEvent::Key('\n'),
                            keyboard::Key::Backspace => gui::InputEvent::Key('\u{8}'),
                            keyboard::Key::Tab => gui::InputEvent::Key('\t'),
                            keyboard::Key::F1 => unreachable!(),
                        };
                        desktop.handle(&event);
                        console.render_desktop(&desktop);
                    } else {
                        shell.handle_key(key, &mut console);
                    }
                }
            }

            network::poll();
            syscall::service_pending();
            task::preemption_point();
            x86_64::instructions::hlt();
        }
    }
    if gpt::self_test() {
        console.println("GPT PARTITIONS: VALIDATED");
    } else {
        console.println("GPT PARTITIONS: VALIDATION FAILED");
        halt();
    }
    if partition::self_test() {
        console.println("MBR PARTITIONS: VALIDATED");
    } else {
        console.println("MBR PARTITIONS: VALIDATION FAILED");
        halt();
    }
    if block::self_test() {
        console.println("BLOCK DEVICE I/O: OK");
    } else {
        console.println("BLOCK DEVICE I/O: FAILED");
        halt();
    }
    if block_cache::self_test() {
        console.println("BLOCK CACHE: OK");
        serial::write_line(format_args!("[BUFFER CACHE] regression tests: PASSED"));
    } else {
        console.println("BLOCK CACHE: FAILED");
        serial::write_line(format_args!("[BUFFER CACHE] regression tests: FAILED"));
        halt();
    }
    if block_io::self_test() {
        console.println("ASYNC BLOCK I/O: OK");
        serial::write_line(format_args!("[BLOCK IO] async completion tests: PASSED"));
    } else {
        console.println("ASYNC BLOCK I/O: FAILED");
        serial::write_line(format_args!("[BLOCK IO] async completion tests: FAILED"));
        halt();
    }
    if swap::self_test() {
        console.println("SWAP BACKING: OK");
        serial::write_line(format_args!("[SWAP] disk-backed policy tests: PASSED"));
    } else {
        console.println("SWAP BACKING: FAILED");
        serial::write_line(format_args!("[SWAP] disk-backed policy tests: FAILED"));
        halt();
    }

    if page_cache::self_test() {
        serial::write_line(format_args!("[FILE PAGES] regression tests: PASSED"));
    } else {
        serial::write_line(format_args!("[FILE PAGES] regression tests: FAILED"));
        halt();
    }

    if ata::self_test() {
        console.println("ATA IDENTIFY PARSER: OK");
    } else {
        console.println("ATA IDENTIFY PARSER: FAILED");
        halt();
    }

    if fat32::self_test() {
        console.println("FAT32 CHAIN READS: OK");
    } else {
        console.println("FAT32 VALIDATION: FAILED");
        halt();
    }
    if network::self_test() {
        console.println("NETWORK/SMOLTCP ADAPTER: OK");
    } else {
        console.println("NETWORK/SMOLTCP ADAPTER: FAILED");
        halt();
    }
    match virtio_net::probe() {
        virtio_net::ProbeStatus::Found(_) => console.println("VIRTIO-NET PCI: DETECTED"),
        virtio_net::ProbeStatus::Missing => console.println("VIRTIO-NET PCI: NOT PRESENT"),
    }

    if vfs::self_test() {
        console.println("VFS READ/WRITE: OK");
        serial::write_line(format_args!("[VFS] read/write and path semantics: PASSED"));
    } else {
        console.println("VFS READ/WRITE: FAILED");
        serial::write_line(format_args!("[VFS] read/write and path semantics: FAILED"));
        halt();
    }
    if paging::frame_ownership_self_test() {
        serial::write_line(format_args!(
            "[FRAME OWNERSHIP] overflow and exhaustion rollback: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[FRAME OWNERSHIP] exhaustion rollback: FAILED"
        ));
        halt();
    }
    if file_frames::self_test() {
        serial::write_line(format_args!(
            "[FRAME CACHE] pinned aliases, LRU eviction, reclaim: PASSED"
        ));
    } else {
        serial::write_line(format_args!("[FRAME CACHE] regression tests: FAILED"));
        halt();
    }
    if userspace::shared_file_mmap_self_test() {
        serial::write_line(format_args!(
            "[SHARED FILE MMAP] aliases, COW, truncation, unlink, reclaim: PASSED"
        ));
    } else {
        serial::write_line(format_args!("[SHARED FILE MMAP] regression tests: FAILED"));
        halt();
    }
    if userspace::lazy_file_mmap_self_test() {
        serial::write_line(format_args!("[LAZY FILE MMAP] regression tests: PASSED"));
    } else {
        serial::write_line(format_args!("[LAZY FILE MMAP] regression tests: FAILED"));
        halt();
    }
    if userspace::private_lazy_swap_self_test() {
        serial::write_line(format_args!(
            "[PRIVATE SWAP MMAP] dirty eviction/refault: PASSED"
        ));
    } else {
        serial::write_line(format_args!("[PRIVATE SWAP MMAP] regression tests: FAILED"));
        halt();
    }
    if userspace::file_mmap_self_test() {
        serial::write_line(format_args!("[FILE MMAP] regression tests: PASSED"));
    } else {
        serial::write_line(format_args!("[FILE MMAP] regression tests: FAILED"));
        halt();
    }
    // Blocking pipe APIs consult the current task, even for immediate reads.
    gdt::init();
    let _user_segments = gdt::user_segments();
    console.println("GDT/TSS: INSTALLED");
    console.println("USER MODE SEGMENTS: READY");

    //
    // Interrupt Descriptor Table
    //

    interrupts::init();

    console.println("IDT: INSTALLED");
    if interrupts::fault_policy_self_test() {
        console.println("USER FAULT RECOVERY: ARMED");
    } else {
        console.println("USER FAULT RECOVERY: FAILED");
        halt();
    }

    task::init();
    console.println("SCHEDULER: INITIALIZED");
    if pipe::self_test() {
        console.println("PIPE: OK");
    } else {
        console.println("PIPE: FAILED");
        halt();
    }
    if storage::self_test() {
        console.println("STORAGE MOUNT PATHS: OK");
    } else {
        console.println("STORAGE MOUNT PATHS: FAILED");
        halt();
    }
    if userspace::elf_loader_self_test() {
        console.println("ELF64 W^X + STACK GUARD: OK");
    } else {
        console.println("ELF64 LOADER VALIDATION: FAILED");
        halt();
    }

    if keyboard::self_test() {
        console.println("KEYBOARD DECODER: OK");
    } else {
        console.println("KEYBOARD DECODER: FAILED");
        halt();
    }
    if gui::self_test() {
        console.println("GUI INPUT: OK");
    } else {
        console.println("GUI INPUT: FAILED");
        halt();
    }
    if ipc::self_test() && ipc::endpoint_count() == 0 {
        console.println("IPC QUEUES: VALIDATED");
    } else {
        console.println("IPC QUEUES: VALIDATION FAILED");
        halt();
    }
    if ipc::handle_object_self_test() && ipc::object_count() == 0 {
        console.println("[S8.1] IPC object/handle local invariants: PASSED");
    } else {
        console.println("[S8.1] IPC object/handle local invariants: FAILED");
        halt();
    }

    if audit::self_test() && task::credential_policy_valid() {
        console.println("CREDENTIAL/AUDIT POLICY: OK");
    } else {
        console.println("CREDENTIAL/AUDIT POLICY: FAILED");
        halt();
    }

    if benchmark::self_test() {
        console.println("BENCHMARK DELTAS: VALIDATED");
    } else {
        console.println("BENCHMARK DELTAS: FAILED");
        halt();
    }
    if syscall::test() {
        console.println("SYSCALL GATE: GETPID OK");
    } else {
        console.println("SYSCALL GATE: FAILED");
        halt();
    }

    if task::capability_policy_valid() {
        console.println("CAPABILITY POLICY: ONLINE");
    } else {
        console.println("CAPABILITY POLICY: FAILED");
        halt();
    }

    if task::capability_delegation_valid() {
        console.println("CAPABILITY DELEGATION: OK");
    } else {
        console.println("CAPABILITY DELEGATION: FAILED");
        halt();
    }

    if audit::count() < 2
        || !audit::latest()
            .is_some_and(|event| event.action == audit::Action::CapabilityRevoke && event.allowed)
    {
        console.println("CAPABILITY AUDIT: FAILED");
        halt();
    }

    pic::init();
    console.println("PIC: INITIALIZED (ALL IRQS MASKED)");

    timer::init();
    let ata_sectors = ata::init();
    if ata_sectors.is_some()
        && !ata::with_primary_master(|disk| {
            let mut sector = [0_u8; block::SECTOR_SIZE];
            block::BlockDevice::read_sector(disk, 0, &mut sector).is_ok()
        })
        .unwrap_or(false)
    {
        console.println("ATA LBA0 READ: FAILED");
        halt();
    }
    let storage_status = storage::mount_ata_root();
    match storage_status {
        storage::MountStatus::Mounted(files) => {
            console.println("FAT32 ROOT MOUNTED");
            serial::write_line(format_args!("[VFS] mounted {} FAT32 root files", files));
        }
        storage::MountStatus::NoDevice => console.println("FAT32 MOUNT: NO BLOCK DEVICE"),
        storage::MountStatus::NotFat32 => console.println("FAT32 MOUNT: NO VOLUME"),
        storage::MountStatus::Failed => console.println("FAT32 MOUNT: FAILED"),
    }
    if userspace::install_stub_executable() {
        console.println("EXEC IMAGE: INSTALLED");
    } else {
        console.println("EXEC IMAGE: INSTALL FAILED");
        halt();
    }
    if userspace::install_init_executable() {
        console.println("INIT IMAGE: INSTALLED");
    } else {
        console.println("INIT IMAGE: INSTALL FAILED");
        halt();
    }
    if userspace::install_shell_executable() {
        console.println("SHELL IMAGE: INSTALLED");
    } else {
        console.println("SHELL IMAGE: INSTALL FAILED");
        halt();
    }
    if userspace::install_echo_executable() {
        console.println("ECHO IMAGE: INSTALLED");
    } else {
        console.println("ECHO IMAGE: INSTALL FAILED");
        halt();
    }
    if userspace::install_true_executable()
        && userspace::install_false_executable()
        && userspace::install_cat_executable()
        && userspace::install_ls_executable()
        && userspace::install_sleep_executable()
        && userspace::install_pwd_executable()
        && userspace::install_mkdir_executable()
        && userspace::install_rm_executable()
    {
        console.println("BIN UTILS: INSTALLED");
    } else {
        console.println("BIN UTILS: INSTALL FAILED");
        halt();
    }
    let vfs_nodes_before_userspace = vfs::node_count();
    let boot_devices = [
        device::Device {
            name: "framebuffer-console",
            kind: device::DeviceKind::Console,
            irq: None,
        },
        device::Device {
            name: "com1",
            kind: device::DeviceKind::Serial,
            irq: None,
        },
        device::Device {
            name: "pit",
            kind: device::DeviceKind::Timer,
            irq: Some(timer::IRQ),
        },
        device::Device {
            name: "ps2-keyboard",
            kind: device::DeviceKind::Keyboard,
            irq: Some(keyboard::IRQ),
        },
    ];
    for device in boot_devices {
        if device::register(device).is_err() {
            console.println("DEVICE REGISTRATION: FAILED");
            halt();
        }
    }
    if ata_sectors.is_some()
        && device::register(device::Device {
            name: "ata0",
            kind: device::DeviceKind::Block,
            irq: None,
        })
        .is_err()
    {
        console.println("ATA DEVICE REGISTRATION: FAILED");
        halt();
    }
    if device::self_test(ata_sectors.is_some()) {
        if let Some(sectors) = ata_sectors {
            console.println("DEVICE REGISTRY: 5 DEVICES ONLINE");
            serial::write_line(format_args!("[ATA] primary master: {} sectors", sectors));
        } else {
            console.println("DEVICE REGISTRY: 4 DEVICES ONLINE (NO ATA)");
        }
    } else {
        console.println("DEVICE REGISTRY: VALIDATION FAILED");
        halt();
    }
    smp::start(acpi.ok(), physical_memory_offset.unwrap());
    if !smp::routed_irq() {
        pic::unmask(timer::IRQ);
        pic::unmask(keyboard::IRQ);
    }
    x86_64::instructions::interrupts::enable();
    #[cfg(feature = "qemu-test")]
    if !heap::runtime_growth_self_test() {
        serial::write_line(format_args!("[S6.HEAP] runtime growth: FAILED"));
        qemu_test_exit_failure();
    }
    #[cfg(feature = "qemu-test")]
    serial::write_line(format_args!(
        "[S6.HEAP] runtime growth: PASSED mapped={} bytes",
        heap::stats().size
    ));
    if ipc::stage8_1_runtime_probe()
        && ipc::object_count() == 0
        && ipc::endpoint_count() == 0
    {
        serial::write_line(format_args!(
            "[S8.1] kernel endpoint objects + process-local handles: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S8.1] kernel endpoint objects + process-local handles: FAILED"
        ));
        halt();
    }
    if ipc::stage8_2_runtime_probe()
        && ipc::object_count() == 0
        && ipc::endpoint_count() == 0
    {
        serial::write_line(format_args!(
            "[S8.2] handle-addressed bounded message passing: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S8.2] handle-addressed bounded message passing: FAILED"
        ));
        halt();
    }
    if ipc::stage8_3_runtime_probe()
        && ipc::object_count() == 0
        && ipc::endpoint_count() == 0
    {
        serial::write_line(format_args!(
            "[S8.3] race-free blocking IPC events/waits: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S8.3] race-free blocking IPC events/waits: FAILED"
        ));
        halt();
    }
    if ipc::stage8_4_runtime_probe()
        && ipc::object_count() == 0
        && ipc::shared_memory_count() == 0
        && ipc::endpoint_count() == 0
    {
        serial::write_line(format_args!(
            "[S8.4] shared-memory objects + cross-address-space mappings: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S8.4] shared-memory objects + cross-address-space mappings: FAILED"
        ));
        halt();
    }
    if ipc::stage8_5_runtime_probe()
        && ipc::object_count() == 0
        && ipc::shared_memory_count() == 0
        && ipc::endpoint_count() == 0
    {
        serial::write_line(format_args!(
            "[S8.5] capability handle transfer through IPC: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S8.5] capability handle transfer through IPC: FAILED"
        ));
        halt();
    }
    if ipc::stage8_6_runtime_probe()
        && ipc::object_count() == 0
        && ipc::shared_memory_count() == 0
        && ipc::endpoint_count() == 0
        && ipc::service_count() == 0
    {
        serial::write_line(format_args!(
            "[S8.6] named service registry + capability discovery: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S8.6] named service registry + capability discovery: FAILED"
        ));
        halt();
    }
    if ipc::stage8_7_runtime_probe()
        && ipc::object_count() == 0
        && ipc::shared_memory_count() == 0
        && ipc::endpoint_count() == 0
        && ipc::service_count() == 0
    {
        serial::write_line(format_args!(
            "[S8.7] multicore IPC/service/capability stress: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S8.7] multicore IPC/service/capability stress: FAILED"
        ));
        halt();
    }
    if wovenguard::self_test() && task::wovenguard_domain_policy_valid() {
        serial::write_line(format_args!(
            "[S9.1] WovenGuard domains + least-privilege policy: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.1] WovenGuard domains + least-privilege policy: FAILED"
        ));
        halt();
    }
    if wovenguard::lineage_self_test() && wovenguard::lineage_count() == 0 {
        serial::write_line(format_args!(
            "[S9.2A] WovenGuard capability lineage foundation: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.2A] WovenGuard capability lineage foundation: FAILED"
        ));
        halt();
    }
    if wovenguard::revocation_self_test() && wovenguard::lineage_count() == 0 {
        serial::write_line(format_args!(
            "[S9.2B] WovenGuard recursive capability revocation: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.2B] WovenGuard recursive capability revocation: FAILED"
        ));
        halt();
    }
    if task::capability_lineage_enforcement_valid() {
        serial::write_line(format_args!(
            "[S9.2C] WovenGuard task capability lineage enforcement: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.2C] WovenGuard task capability lineage enforcement: FAILED"
        ));
        halt();
    }
    if ipc::stage9_2d_runtime_probe() {
        serial::write_line(format_args!(
            "[S9.2D] WovenGuard IPC/object capability lineage: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.2D] WovenGuard IPC/object capability lineage: FAILED"
        ));
        halt();
    }
    if ipc::stage9_2e_runtime_probe() {
        serial::write_line(format_args!(
            "[S9.2E] WovenGuard SMP revocation + lineage closure: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.2E] WovenGuard SMP revocation + lineage closure: FAILED"
        ));
        halt();
    }
    if audit::stage9_3_runtime_probe() {
        serial::write_line(format_args!(
            "[S9.3] WovenGuard security audit ledger: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.3] WovenGuard security audit ledger: FAILED"
        ));
        halt();
    }
    if wovenguard::self_test() && task::sandbox_profile_foundation_valid() {
        serial::write_line(format_args!(
            "[S9.4A] WovenGuard sandbox profile + capability ceiling: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.4A] WovenGuard sandbox profile + capability ceiling: FAILED"
        ));
        halt();
    }
    if ipc::stage9_4b_runtime_probe() {
        serial::write_line(format_args!(
            "[S9.4B] WovenGuard IPC/service exposure policy: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.4B] WovenGuard IPC/service exposure policy: FAILED"
        ));
        halt();
    }
    if task::filesystem_sandbox_foundation_valid() {
        serial::write_line(format_args!(
            "[S9.4C] WovenGuard filesystem/resource sandbox: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.4C] WovenGuard filesystem/resource sandbox: FAILED"
        ));
        halt();
    }
    if task::sandbox_lifecycle_smp_closure_valid() {
        serial::write_line(format_args!(
            "[S9.4D] WovenGuard sandbox lifecycle + SMP closure: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.4D] WovenGuard sandbox lifecycle + SMP closure: FAILED"
        ));
        halt();
    }
    if task::device_capability_gates_valid() {
        serial::write_line(format_args!(
            "[S9.5] WovenGuard resource/device capability gates: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S9.5] WovenGuard resource/device capability gates: FAILED"
        ));
        halt();
    }
    // The root `network-test` mode always enables `qemu-test`.  Cargo artifact
    // dependencies can keep a nested kernel-only feature isolated, so key the
    // runtime probe from the qemu-test feature that is known to reach this
    // kernel artifact.  Only QEMU configurations that actually expose the
    // supported VirtIO network device run the live networking regression.
    //
    // Memory/storage QEMU suites do not attach that VirtIO NIC: `network::init`
    // simply fails its transport probe and those suites continue normally.
    #[cfg(all(
        feature = "qemu-test",
        not(feature = "stage10-6-test"),
        not(feature = "stage10-7-test")
    ))]
    {
        match network::init() {
            Ok(()) => {
                serial::write_line(format_args!(
                    "[NETTEST] virtio-net + smoltcp initialized"
                ));
                if network::qemu_runtime_self_test() {
                    serial::write_line(format_args!(
                        "[NETTEST] DHCP/DNS/ICMP/UDP/TCP: PASSED"
                    ));
                } else {
                    serial::write_line(format_args!(
                        "[NETTEST] runtime regression: FAILED"
                    ));
                    halt();
                }
            }
            Err(error) => {
                // A network-test image must never silently downgrade to the
                // no-NIC memory/storage path: the host harness relies on this
                // marker to distinguish transport failure from an intentional
                // skip. Other QEMU images still omit the NIC and continue.
                #[cfg(feature = "network-test")]
                {
                    serial::write_line(format_args!("[NETTEST] init failed: {:?}", error));
                    halt();
                }
                #[cfg(not(feature = "network-test"))]
                {
                    let _ = error;
                }
            }
        }
    }
    if !block_io::start_worker() {
        console.println("BLOCK I/O WORKER: START FAILED");
        halt();
    }
    #[cfg(feature = "stage10-9-test")]
    if !async_events::start_worker() {
        console.println("TIMER/EVENT WORKER: START FAILED");
        halt();
    }
    if block_io::async_completion_self_test() {
        serial::write_line(format_args!("[BLOCK IO] worker completion: PASSED"));
    } else {
        serial::write_line(format_args!("[BLOCK IO] worker completion: FAILED"));
        halt();
    }
    if block_io::stage10_1_event_driven_completion_valid() {
        serial::write_line(format_args!(
            "[S10.1] kernel event + async I/O foundation: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S10.1] kernel event + async I/O foundation: FAILED"
        ));
        halt();
    }
    if block_io::stage10_2_generic_async_completion_valid() {
        serial::write_line(format_args!(
            "[S10.2] generic async request/completion API: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S10.2] generic async request/completion API: FAILED"
        ));
        halt();
    }
    if !task::start_pager() {
        console.println("PAGER: START FAILED");
        halt();
    }
    match storage::live_mutation_self_test() {
        storage::LiveMutationTestStatus::Passed => {
            serial::write_line(format_args!(
                "[STORAGE MUTATION] live FAT32 rename/delete/growth/lifecycle: PASSED"
            ));
        }
        storage::LiveMutationTestStatus::Skipped => {
            serial::write_line(format_args!(
                "[STORAGE MUTATION] live FAT32 rename/delete/growth/lifecycle: SKIPPED"
            ));
        }
        storage::LiveMutationTestStatus::Failed(stage) => {
            serial::write_line(format_args!(
                "[STORAGE MUTATION] live FAT32 rename/delete/growth/lifecycle: FAILED at {}",
                stage
            ));
            halt();
        }
    }

    while timer::ticks() < 3 {
        task::yield_now();
    }

    let probe_id = match task::spawn("preemption-probe", preemption_probe_task) {
        Ok(id) => id,
        Err(_) => {
            console.println("PREEMPTION TEST: SPAWN FAILED");
            halt();
        }
    };

    while !PREEMPTION_PROBE_BLOCKED.load(Ordering::Acquire) {
        x86_64::instructions::hlt();
    }

    if !task::wake_task(probe_id) {
        console.println("TASK WAKEUP: FAILED");
        halt();
    }

    while !PREEMPTION_PROBE_COMPLETED.load(Ordering::Acquire) {
        x86_64::instructions::hlt();
    }

    console.println("TIMER IRQ: OK");
    console.println("TIMER PREEMPTION: OK");
    console.println("TASK SLEEP/BLOCK/WAKE: OK");
    serial::write_line(format_args!(
        "[BOOT] timer preemption and task lifecycle verified"
    ));

    let preemptions_before_fairness = task::summary().preemption_switches;
    if task::spawn("fair-peer-a", fairness_probe_a).is_err() {
        console.println("SCHEDULER FAIRNESS A: SPAWN FAILED");
        serial::write_line(format_args!("[SCHED] peer A spawn failed"));
        halt();
    }
    if task::spawn("fair-peer-b", fairness_probe_b).is_err() {
        console.println("SCHEDULER FAIRNESS B: SPAWN FAILED");
        serial::write_line(format_args!("[SCHED] peer B spawn failed"));
        halt();
    }
    while FAIR_TASKS_COMPLETED.load(Ordering::Acquire) != 2 {
        x86_64::instructions::hlt();
    }
    let fairness_summary = task::summary();
    if FAIR_TASK_A_RUNS.load(Ordering::Acquire) == 0
        || FAIR_TASK_B_RUNS.load(Ordering::Acquire) == 0
        || fairness_summary.preemption_switches < preemptions_before_fairness + 2
    {
        console.println("SCHEDULER FAIRNESS/QUANTUM: FAILED");
        halt();
    }
    console.println("PRIORITY ROUND-ROBIN/TIME SLICES: OK");
    serial::write_line(format_args!(
        "[BOOT] priority round-robin fairness and per-task quanta verified"
    ));

    console.println("TIMER IRQ: OK");

    keyboard::inject_validation_input(2);
    let isolation_baseline = memory::stats().allocated_frames;
    let Some(first_program) = userspace::create_exec_process() else {
        console.println("USER PROCESS IMAGE: MAPPING FAILED");
        halt();
    };
    let Some(second_program) = userspace::create_stub_process() else {
        console.println("SECOND USER ADDRESS SPACE: FAILED");
        halt();
    };
    if !first_program.stack.is_aligned()
        || first_program.stack.size != userspace::UserStack::SIZE
        || !paging::user_range_is_unmapped_in(
            first_program.address_space.paging(),
            first_program.stack.guard_base,
            userspace::UserStack::GUARD_SIZE,
        )
        || !paging::user_range_has_protection_in(
            first_program.address_space.paging(),
            first_program.stack.base,
            first_program.stack.size,
            true,
            false,
        )
        || !paging::user_range_is_unmapped_in(
            second_program.address_space.paging(),
            second_program.stack.guard_base,
            userspace::UserStack::GUARD_SIZE,
        )
        || first_program.image.entry != second_program.image.entry
        || first_program.stack.top != second_program.stack.top
        || first_program.address_space.root_address() == second_program.address_space.root_address()
    {
        console.println("USER ADDRESS-SPACE ISOLATION: INVALID");
        halt();
    }
    serial::write_line(format_args!(
        "[BOOT] independent user roots, unmapped stack guards, and RW/NX stacks verified"
    ));

    if !userspace::mmap_w_xor_x_self_test(first_program.address_space) {
        console.println("MMAP W^X INVARIANT: FAILED");
        halt();
    }
    console.println("MMAP W^X INVARIANT: OK");
    serial::write_line(format_args!(
        "[BOOT] anonymous mmap W^X invariant verified (writable mapping is never executable)"
    ));

    if !task::file_fault_io_self_test() {
        serial::write_line(format_args!(
            "[FAULT IO] interrupt/preemption guard: FAILED"
        ));
        halt();
    }
    serial::write_line(format_args!(
        "[FAULT IO] timer IRQs live, task stable, state restored: PASSED"
    ));
    let first_root = first_program.address_space.root_address();
    let second_root = second_program.address_space.root_address();
    // Publish the paired bootstrap processes as one BSP-local transaction.
    // A timer interrupt used to be able to schedule process A after its TCB was
    // made Ready but before process B and its process-table entry were created.
    // That timing window became visible with multiple LAPIC timers running and
    // could leave the boot validation waiting forever before the identity audit.
    // APs may continue taking timer interrupts, but their scheduler path uses
    // try_lock and cannot run CPU-0-owned userspace tasks.
    serial::write_line(format_args!(
        "[BOOT] paired userspace spawn: BEGIN"
    ));
    let (first_spawn, second_spawn) = x86_64::instructions::interrupts::without_interrupts(|| {
        let first = task::spawn_user_process("init-user-a", first_program);
        let second = if first.is_ok() {
            Some(task::spawn_user_process("init-user-b", second_program))
        } else {
            None
        };
        (first, second)
    });

    let first_pid = match first_spawn {
        Ok((pid, context)) => {
            serial::write_line(format_args!(
                "[BOOT] ring3 frame CS={:#x} SS={:#x} RIP={:#x} RSP={:#x}",
                context.code_segment, context.data_segment, context.entry, context.stack_top,
            ));
            pid
        }
        Err(_) => {
            console.println("FIRST USER PROCESS: SPAWN FAILED");
            halt();
        }
    };
    let second_pid = match second_spawn {
        Some(Ok((pid, _))) => pid,
        _ => {
            console.println("SECOND USER PROCESS: SPAWN FAILED");
            halt();
        }
    };
    serial::write_line(format_args!(
        "[BOOT] paired userspace spawn: READY"
    ));
    serial::write_line(format_args!(
        "[PROC-DIAG] first credentials lookup: BEGIN pid={}",
        first_pid.as_u64()
    ));
    let first_credentials = task::process_credentials(first_pid);
    serial::write_line(format_args!(
        "[PROC-DIAG] first credentials lookup: DONE"
    ));

    serial::write_line(format_args!(
        "[PROC-DIAG] second credentials lookup: BEGIN pid={}",
        second_pid.as_u64()
    ));
    let second_credentials = task::process_credentials(second_pid);
    serial::write_line(format_args!(
        "[PROC-DIAG] second credentials lookup: DONE"
    ));

    if first_credentials != Some(task::Credentials::USERSPACE)
        || second_credentials != Some(task::Credentials::USERSPACE)
    {
        console.println("USER CREDENTIALS: INVALID");
        halt();
    }
    serial::write_line(format_args!(
        "[AUDIT] userspace identity uid={} gid={}",
        task::Credentials::USERSPACE.uid,
        task::Credentials::USERSPACE.gid,
    ));

    serial::write_line(format_args!(
        "[PROC-DIAG] paired userspace execution wait: BEGIN"
    ));
    let user_pair_wait_start = timer::ticks();
    while !task::process_exited(first_pid) || !task::process_exited(second_pid) {
        if timer::ticks().wrapping_sub(user_pair_wait_start) > 2_000 {
            serial::write_line(format_args!(
                "[BOOT] paired userspace execution: TIMEOUT first_exited={} second_exited={}",
                task::process_exited(first_pid),
                task::process_exited(second_pid),
            ));
            console.println("PAIRED USERSPACE EXECUTION: TIMEOUT");
            halt();
        }
        x86_64::instructions::hlt();
    }
    serial::write_line(format_args!(
        "[BOOT] paired userspace execution: PASSED"
    ));

    // Stage 7.3: prove that the already-established per-CPU privilege and
    // scheduler machinery can execute a deliberately pinned Ring-3 task on
    // every application processor. Keep the workload intentionally minimal:
    // `/bin/true` performs only exit(0), so this test does not relax Stage 7.2's
    // BSP ownership of filesystem/network/pager/device service execution.
    if smp::online_count() > 1 {
        serial::write_line(format_args!(
            "[S7.3] pinned Ring-3 AP execution: BEGIN online={}",
            smp::online_count()
        ));
        for cpu in 1..smp::online_count() {
            let Some(program) = userspace::create_true_process() else {
                serial::write_line(format_args!(
                    "[S7.3] cpu={} true image creation: FAILED",
                    cpu
                ));
                halt();
            };
            let pid = match task::spawn_pinned_user_process_on(cpu, "s7.3-ring3", program) {
                Ok((pid, _)) => pid,
                Err(_) => {
                    serial::write_line(format_args!(
                        "[S7.3] cpu={} pinned Ring-3 spawn: FAILED",
                        cpu
                    ));
                    halt();
                }
            };

            let start = timer::ticks();
            while !task::process_exited(pid) {
                if timer::ticks().wrapping_sub(start) > 500 {
                    serial::write_line(format_args!(
                        "[S7.3] cpu={} pinned Ring-3 execution: TIMEOUT pid={}",
                        cpu,
                        pid.as_u64()
                    ));
                    halt();
                }
                x86_64::instructions::hlt();
            }
            match task::wait_process(pid.as_u64()) {
                Ok(0) => {}
                Ok(code) => {
                    serial::write_line(format_args!(
                        "[S7.3] cpu={} pinned Ring-3 exit code: FAILED code={}",
                        cpu, code
                    ));
                    halt();
                }
                Err(_) => {
                    serial::write_line(format_args!(
                        "[S7.3] cpu={} pinned Ring-3 reap: FAILED",
                        cpu
                    ));
                    halt();
                }
            }
            serial::write_line(format_args!(
                "[S7.3] cpu={} pinned Ring-3 execution: PASSED",
                cpu
            ));
        }
        serial::write_line(format_args!(
            "[S7.3] pinned Ring-3 execution on every AP: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S7.3] single-CPU baseline preserved; no AP Ring-3 probe required"
        ));
    }

    // Stage 7.4: prove Ready-state Ring-3 ownership migration and hard
    // affinity without enabling general userspace migration. Both probes use
    // `/bin/true`, whose AP syscall/exit path was validated in Stage 7.3.
    if smp::online_count() > 1 {
        let target_cpu = smp::online_count() - 1;
        serial::write_line(format_args!(
            "[S7.4] Ready Ring-3 migration/affinity: BEGIN target_cpu={}",
            target_cpu
        ));

        // Explicit Ready-state migration: create on CPU0 with an all-online
        // affinity mask, move ownership under the scheduler lock, then let the
        // destination CPU perform the first dispatch.
        let Some(program) = userspace::create_true_process() else {
            serial::write_line(format_args!("[S7.4] migration true image creation: FAILED"));
            halt();
        };
        // Keep the Ready-state setup atomic with respect to BSP dispatch.
        // `spawn_migratable_user_probe()` publishes the probe Ready on CPU0;
        // without this local interrupt boundary, a timer tick can dispatch the
        // tiny `/bin/true` probe before the test applies the intended migration.
        // The scheduler's Ready-only contract remains unchanged.
        let (migration_pid, _, _) =
            x86_64::instructions::interrupts::without_interrupts(|| {
                let result = match task::spawn_migratable_user_probe("s7.4-migrate", program) {
                    Ok(result) => result,
                    Err(_) => {
                        serial::write_line(format_args!(
                            "[S7.4] migratable Ring-3 spawn: FAILED"
                        ));
                        halt();
                    }
                };
                if task::task_affinity(result.1) != Some(smp::online_mask()) {
                    serial::write_line(format_args!(
                        "[S7.4] initial migratable affinity: FAILED"
                    ));
                    halt();
                }
                if task::migrate_ready_task(result.1, target_cpu).is_err() {
                    serial::write_line(format_args!(
                        "[S7.4] explicit Ready migration to cpu={}: FAILED",
                        target_cpu
                    ));
                    halt();
                }
                result
            });
        let migration_start = timer::ticks();
        while !task::process_exited(migration_pid) {
            if timer::ticks().wrapping_sub(migration_start) > 500 {
                serial::write_line(format_args!(
                    "[S7.4] explicit Ready migration execution: TIMEOUT pid={} cpu={}",
                    migration_pid.as_u64(),
                    target_cpu
                ));
                halt();
            }
            x86_64::instructions::hlt();
        }
        if task::wait_process(migration_pid.as_u64()) != Ok(0) {
            serial::write_line(format_args!("[S7.4] explicit Ready migration reap: FAILED"));
            halt();
        }
        serial::write_line(format_args!(
            "[S7.4] explicit Ready Ring-3 migration 0->{}: PASSED",
            target_cpu
        ));

        // Affinity-driven ownership transfer: excluding CPU0 from a Ready
        // migratable task must atomically move ownership to the lowest allowed
        // CPU before it can run. A single-bit mask makes the destination exact.
        let Some(program) = userspace::create_true_process() else {
            serial::write_line(format_args!("[S7.4] affinity true image creation: FAILED"));
            halt();
        };
        let target_mask = 1usize << target_cpu;
        // As above, do not expose a dispatch window between publishing the
        // CPU0-owned Ready probe and applying its hard affinity. Once the
        // affinity call moves ownership to the AP, normal SMP execution resumes.
        let (affinity_pid, _, _) =
            x86_64::instructions::interrupts::without_interrupts(|| {
                let result = match task::spawn_migratable_user_probe("s7.4-affinity", program) {
                    Ok(result) => result,
                    Err(_) => {
                        serial::write_line(format_args!(
                            "[S7.4] affinity Ring-3 spawn: FAILED"
                        ));
                        halt();
                    }
                };
                if task::set_ready_task_affinity(result.1, target_mask).is_err() {
                    serial::write_line(format_args!(
                        "[S7.4] hard affinity transfer to cpu={}: FAILED",
                        target_cpu
                    ));
                    halt();
                }
                result
            });
        let affinity_start = timer::ticks();
        while !task::process_exited(affinity_pid) {
            if timer::ticks().wrapping_sub(affinity_start) > 500 {
                serial::write_line(format_args!(
                    "[S7.4] hard-affinity Ring-3 execution: TIMEOUT pid={} cpu={}",
                    affinity_pid.as_u64(),
                    target_cpu
                ));
                halt();
            }
            x86_64::instructions::hlt();
        }
        if task::wait_process(affinity_pid.as_u64()) != Ok(0) {
            serial::write_line(format_args!("[S7.4] hard-affinity Ring-3 reap: FAILED"));
            halt();
        }
        serial::write_line(format_args!(
            "[S7.4] hard-affinity Ring-3 transfer to cpu={}: PASSED",
            target_cpu
        ));
        serial::write_line(format_args!(
            "[S7.4] Ready-state Ring-3 migration + affinity: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S7.4] single-CPU baseline preserved; no Ring-3 migration required"
        ));
    }

    // Stage 7.6: integrate the validated Ring-3 placement/migration pieces
    // without changing legacy CPU0-owned userspace. Audited multicore tasks get
    // least-loaded placement and may participate in the existing Ready-only
    // rebalancer. The burst below intentionally starts on CPU0 through the
    // Stage 7.4 probe API so automatic userspace rebalancing has real work.
    if smp::online_count() > 1 {
        serial::write_line(format_args!(
            "[S7.6] integrated multicore userspace: BEGIN online={}",
            smp::online_count()
        ));

        let mut balance_pids = [None; 4];
        let before_rebalance = task::rebalance_stats();
        let before_userspace_rebalance = task::userspace_rebalance_migrations();
        x86_64::instructions::interrupts::without_interrupts(|| {
            for slot in &mut balance_pids {
                let Some(program) = userspace::create_true_process() else {
                    serial::write_line(format_args!(
                        "[S7.6] rebalance probe image creation: FAILED"
                    ));
                    halt();
                };
                let (pid, _, _) = match task::spawn_migratable_user_probe(
                    "s7.6-rebalance",
                    program,
                ) {
                    Ok(result) => result,
                    Err(_) => {
                        serial::write_line(format_args!(
                            "[S7.6] rebalance probe spawn: FAILED"
                        ));
                        halt();
                    }
                };
                *slot = Some(pid);
            }
            if !task::rebalance_once() {
                serial::write_line(format_args!(
                    "[S7.6] automatic Ready Ring-3 rebalance: FAILED"
                ));
                halt();
            }
        });
        let after_rebalance = task::rebalance_stats();
        if after_rebalance.migrations <= before_rebalance.migrations
            || task::userspace_rebalance_migrations() <= before_userspace_rebalance
        {
            serial::write_line(format_args!(
                "[S7.6] userspace rebalance migration accounting: FAILED"
            ));
            halt();
        }
        for pid in balance_pids.into_iter().flatten() {
            let start = timer::ticks();
            while !task::process_exited(pid) {
                if timer::ticks().wrapping_sub(start) > 500 {
                    serial::write_line(format_args!(
                        "[S7.6] rebalanced Ring-3 execution: TIMEOUT pid={}",
                        pid.as_u64()
                    ));
                    halt();
                }
                x86_64::instructions::hlt();
            }
            if task::wait_process(pid.as_u64()) != Ok(0) {
                serial::write_line(format_args!(
                    "[S7.6] rebalanced Ring-3 reap: FAILED"
                ));
                halt();
            }
        }
        serial::write_line(format_args!(
            "[S7.6] automatic Ready Ring-3 rebalancing: PASSED"
        ));

        // Exercise the production multicore spawn API. With only the kernel
        // task runnable on CPU0, least-loaded placement must select an AP.
        let Some(program) = userspace::create_true_process() else {
            serial::write_line(format_args!(
                "[S7.6] multicore placement image creation: FAILED"
            ));
            halt();
        };
        let (placement_pid, _placement_task, _, placement) =
            match task::spawn_multicore_user_process(
                "s7.6-placement",
                program,
                smp::online_mask(),
            ) {
                Ok(result) => result,
                Err(_) => {
                    serial::write_line(format_args!(
                        "[S7.6] multicore placement spawn: FAILED"
                    ));
                    halt();
                }
            };
        let owner_cpu = placement.owner_cpu;
        if owner_cpu == 0 || placement.affinity_mask != smp::online_mask() {
            serial::write_line(format_args!(
                "[S7.6] least-loaded Ring-3 placement: FAILED owner={} affinity={:#x} expected={:#x}",
                owner_cpu,
                placement.affinity_mask,
                smp::online_mask(),
            ));
            halt();
        }
        let start = timer::ticks();
        while !task::process_exited(placement_pid) {
            if timer::ticks().wrapping_sub(start) > 500 {
                serial::write_line(format_args!(
                    "[S7.6] least-loaded Ring-3 execution: TIMEOUT pid={} cpu={}",
                    placement_pid.as_u64(),
                    owner_cpu
                ));
                halt();
            }
            x86_64::instructions::hlt();
        }
        if task::wait_process(placement_pid.as_u64()) != Ok(0) {
            serial::write_line(format_args!(
                "[S7.6] least-loaded Ring-3 reap: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S7.6] least-loaded multicore Ring-3 placement: PASSED cpu={}",
            owner_cpu
        ));
    } else {
        serial::write_line(format_args!(
            "[S7.6] single-CPU integrated userspace baseline preserved"
        ));
    }

    if !syscall::user_memory_verified() || task::anonymous_mapping_count() != 0 {
        console.println("USER MMAP/MUNMAP: FAILED");
        halt();
    }
    let async_stats = async_op::stats();
    if !syscall::user_async_verified() || async_stats.active != 0 || async_stats.owner_reaped < 2 {
        serial::write_line(format_args!(
            "[S10.3] userspace async completion ABI: FAILED verified={} active={} owner_reaped={}",
            syscall::user_async_verified(), async_stats.active, async_stats.owner_reaped
        ));
        halt();
    }
    serial::write_line(format_args!(
        "[S10.3] userspace async completion ABI + cancellation/teardown: PASSED"
    ));
    #[cfg(feature = "stage11-1-test")]
    {
        serial::write_line(format_args!("[S11.1] production process model: PASSED"));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage6-hotplug-test")]
    {
        if !smp::hotplug_cancellation_self_test() {
            serial::write_line(format_args!("[S6.HOTPLUG] cancellation: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S6.HOTPLUG] cancellation: PASSED"));
        if !task::hotplug_evacuation_atomic_self_test() {
            serial::write_line(format_args!("[S6.HOTPLUG] evacuation: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S6.HOTPLUG] evacuation: PASSED"));
        if !smp::hotplug_rejection_recovery_probe(smp::online_count().saturating_sub(1)) {
            serial::write_line(format_args!("[S6.HOTPLUG] rejection recovery: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S6.HOTPLUG] rejection recovery: PASSED"));
        if !smp::hotplug_timeout_recovery_probe(smp::online_count().saturating_sub(1)) {
            serial::write_line(format_args!("[S6.HOTPLUG] timeout recovery: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S6.HOTPLUG] timeout recovery: PASSED"));
        const CYCLES: usize = 2;
        for cycle in 0..CYCLES {
            let offline_cpu = smp::online_count().saturating_sub(1);
            if smp::online_count() < 2 || !smp::request_cpu_offline(offline_cpu) {
                serial::write_line(format_args!("[S6.HOTPLUG] offline AP: FAILED cycle={}", cycle));
                qemu_test_exit_failure();
            }
            if cycle == 0 {
                serial::write_line(format_args!(
                    "[S6.HOTPLUG] offline AP: PASSED online={} mask={:#x}",
                    smp::online_count(), smp::online_mask()
                ));
            }
            if !smp::request_cpu_online(offline_cpu) {
                serial::write_line(format_args!("[S6.HOTPLUG] online AP: FAILED cycle={}", cycle));
                qemu_test_exit_failure();
            }
            serial::write_line(format_args!(
                "[S6.HOTPLUG] cycle={} PASSED online={} mask={:#x}",
                cycle + 1, smp::online_count(), smp::online_mask()
            ));
        }
        serial::write_line(format_args!(
            "[S6.HOTPLUG] lifecycle: PASSED cycles={} online={} mask={:#x}",
            CYCLES,
            smp::online_count(), smp::online_mask()
        ));
        if smp::online_count() == 4 {
            for cycle in 0..CYCLES {
                if !smp::request_cpu_offline(1)
                    || smp::online_count() != 3
                    || smp::online_mask() != 0xd
                    || smp::reschedule_cpu(1)
                    || !smp::reschedule_cpu(3)
                {
                    serial::write_line(format_args!(
                        "[S6.HOTPLUG] middle offline: FAILED cycle={}", cycle
                    ));
                    qemu_test_exit_failure();
                }
                if !smp::hotplug_hole_worker_probe(cycle + 1) {
                    serial::write_line(format_args!(
                        "[S6.HOTPLUG] hole worker: FAILED cycle={}", cycle
                    ));
                    qemu_test_exit_failure();
                }
                smp::shootdown();
                if !smp::request_cpu_online(1)
                    || smp::online_count() != 4
                    || smp::online_mask() != 0xf
                {
                    serial::write_line(format_args!(
                        "[S6.HOTPLUG] middle online: FAILED cycle={}", cycle
                    ));
                    qemu_test_exit_failure();
                }
            }
            serial::write_line(format_args!(
                "[S6.HOTPLUG] middle lifecycle: PASSED cycles={} online={} mask={:#x}",
                CYCLES, smp::online_count(), smp::online_mask()
            ));
        }
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage11-2-test")]
    {
        if !thread::structural_self_test() {
            serial::write_line(format_args!("[S11.2] threads/TLS/join: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S11.2] threads/TLS/join: PASSED"));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage11-3-test")]
    {
        if !notifications::structural_self_test() {
            serial::write_line(format_args!("[S11.3] notifications: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S11.3] notifications: PASSED"));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage12-1-test")]
    {
        if !vfs_api::structural_self_test() {
            serial::write_line(format_args!("[S12.1] VFS boundary: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S12.1] VFS boundary: PASSED"));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage13-1-test")]
    { if !driver::register("pit", device::DeviceKind::Timer) || !driver::bind("pit") || !driver::suspend("pit") || !driver::resume("pit") { serial::write_line(format_args!("[S13.1] driver framework: FAILED")); qemu_test_exit_failure(); } serial::write_line(format_args!("[S13.1] driver framework: PASSED")); qemu_test_exit_success(); }
    #[cfg(feature = "stage13-2-test")]
    {
        let pci_ok = hal::pci::self_test()
            && hardware.pci.discovered >= u16::from(hardware.pci.recorded)
            && hardware.pci.recorded != 0;
        if !pci_ok {
            serial::write_line(format_args!("[S13.2] PCI/PCIe configuration + inventory: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!(
            "[S13.2] PCI/PCIe configuration + inventory: PASSED devices={} recorded={} segments={} ecam={}",
            hardware.pci.discovered, hardware.pci.recorded, hardware.pci.segments, hardware.pci.ecam as u8
        ));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage13-3-test")]
    {
        use block::BlockDevice;
        if !nvme::self_test() || !nvme::probe() {
            serial::write_line(format_args!("[S13.3] NVMe controller/queue foundation: FAILED probe"));
            qemu_test_exit_failure();
        }
        let sectors = match nvme::init() {
            Ok(sectors) => sectors,
            Err(error) => {
                serial::write_line(format_args!("[S13.3] NVMe controller/queue foundation: FAILED init {:?}", error));
                qemu_test_exit_failure();
            }
        };
        let io_ok = nvme::with_controller(|controller| {
            let mut original = [0_u8; block::SECTOR_SIZE];
            let mut verify = [0_u8; block::SECTOR_SIZE];
            let mut pattern = [0_u8; block::SECTOR_SIZE];
            pattern[..15].copy_from_slice(b"wovenhat-nvme13");
            if controller.read_sector(0, &mut original).is_err()
                || controller.write_sector(0, &pattern).is_err()
                || controller.flush().is_err()
                || controller.read_sector(0, &mut verify).is_err()
                || verify != pattern
            {
                return false;
            }
            controller.write_sector(0, &original).is_ok() && controller.flush().is_ok()
        })
        .unwrap_or(false);
        if !io_ok {
            serial::write_line(format_args!("[S13.3] NVMe controller/queue foundation: FAILED I/O"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!(
            "[S13.3] NVMe controller/queue foundation: PASSED sectors={}",
            sectors
        ));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage13-4-test")]
    {
        use block::BlockDevice;
        if !ahci::self_test() || !ahci::probe() {
            serial::write_line(format_args!("[S13.4] AHCI/SATA DMA block I/O: FAILED probe"));
            qemu_test_exit_failure();
        }
        let sectors = match ahci::init() {
            Ok(sectors) => sectors,
            Err(error) => {
                serial::write_line(format_args!("[S13.4] AHCI/SATA DMA block I/O: FAILED init {:?}", error));
                qemu_test_exit_failure();
            }
        };
        let io_ok = ahci::with_controller(|controller| {
            let mut original = [0_u8; block::SECTOR_SIZE];
            let mut verify = [0_u8; block::SECTOR_SIZE];
            let mut pattern = [0_u8; block::SECTOR_SIZE];
            pattern[..15].copy_from_slice(b"wovenhat-ahci14");
            if controller.read_sector(0, &mut original).is_err()
                || controller.write_sector(0, &pattern).is_err()
                || controller.flush().is_err()
                || controller.read_sector(0, &mut verify).is_err()
                || verify != pattern
            {
                return false;
            }
            controller.write_sector(0, &original).is_ok() && controller.flush().is_ok()
        })
        .unwrap_or(false);
        if !io_ok {
            serial::write_line(format_args!("[S13.4] AHCI/SATA DMA block I/O: FAILED I/O"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!(
            "[S13.4] AHCI/SATA DMA block I/O: PASSED sectors={}",
            sectors
        ));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage13-5-test")]
    {
        if !xhci::self_test() || !xhci::probe() {
            serial::write_line(format_args!("[S13.5] xHCI USB core: FAILED probe"));
            qemu_test_exit_failure();
        }
        match xhci::init() {
            Ok((slots, ports, connected, slot)) => {
                serial::write_line(format_args!(
                    "[S13.5] xHCI USB core: PASSED slots={} ports={} connected_port={} slot={}",
                    slots, ports, connected, slot
                ));
                qemu_test_exit_success();
            }
            Err(error) => {
                serial::write_line(format_args!("[S13.5] xHCI USB core: FAILED init {:?}", error));
                qemu_test_exit_failure();
            }
        }
    }
    #[cfg(feature = "stage13-8-test")]
    {
        if !hda::self_test() || !woven_audio::self_test() || !hda::probe() {
            serial::write_line(format_args!("[S13.8] WovenAudio HDA foundation: FAILED probe/self-test"));
            qemu_test_exit_failure();
        }
        match woven_audio::init() {
            Ok(_caps) => {
                match hda::discover_codec() {
                    Ok(codec) => serial::write_line(format_args!(
                        "[S13.8] HDA codec command transport: PASSED cad={} vendor={:#010x} revision={:#010x} root_start={} root_count={}",
                        codec.address,
                        codec.vendor_id,
                        codec.revision_id,
                        codec.root_start_node,
                        codec.root_node_count
                    )),
                    Err(error) => {
                        serial::write_line(format_args!("[S13.8] HDA codec command transport: FAILED {:?}", error));
                        qemu_test_exit_failure();
                    }
                }

                match hda::discover_topology() {
                    Ok(topology) => {
                        serial::write_line(format_args!(
                            "[S13.8] HDA codec topology: PASSED afg={} widgets={} dac={} adc={} mixers={} selectors={} pins={}",
                            topology.audio_function_groups,
                            topology.widgets,
                            topology.audio_outputs,
                            topology.audio_inputs,
                            topology.mixers,
                            topology.selectors,
                            topology.pin_complexes
                        ));
                    }
                    Err(error) => {
                        serial::write_line(format_args!("[S13.8] HDA codec topology: FAILED {:?}", error));
                        qemu_test_exit_failure();
                    }
                }

        let audio_api = match woven_audio::integration_smoke_test() {
        Ok(summary) => summary,
        Err(error) => {
            serial::write_line(format_args!(
                "[S13.8] WovenAudio stream/API integration: FAILED {:?}",
                error
            ));
            qemu_test_exit_failure();
        }
    };
    serial::write_line(format_args!(
        "[S13.8] WovenAudio stream/API integration: PASSED playback_stream={} capture_stream={} playback_bytes={} capture_bytes={} rate={} channels={} bits={} gain={} mute={}",
        audio_api.playback.hardware_stream,
        audio_api.capture.hardware_stream,
        audio_api.playback.bytes_transferred,
        audio_api.capture.bytes_transferred,
        audio_api.playback.format.sample_rate_hz,
        audio_api.playback.format.channels,
        audio_api.playback.format.bits_per_sample,
        audio_api.mixer.current_gain,
        audio_api.mixer.mute_supported
    ));
                qemu_test_exit_success();
            }
            Err(error) => {
                serial::write_line(format_args!("[S13.8] WovenAudio HDA foundation: FAILED init {:?}", error));
                qemu_test_exit_failure();
            }
        }
    }
    #[cfg(feature = "stage13-7-test")]
    {
        if !woven_input::self_test() || !keyboard::self_test() {
            serial::write_line(format_args!("[S13.7] WovenInput framework: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!(
            "[S13.7] WovenInput unified event framework: PASSED dropped={}",
            woven_input::dropped_events()
        ));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage13-6-test")]
    {
        if !xhci::self_test() || !xhci::probe() {
            serial::write_line(format_args!("[S13.6] USB HID: FAILED probe/self-test"));
            qemu_test_exit_failure();
        }
        match xhci::init_hid() {
            Ok(summary) => {
                let report = match xhci::poll_hid_report() {
                    Ok(report) => report,
                    Err(error) => {
                        match hda::discover_topology() {
                        Ok(topology) => {
                            serial::write_line(format_args!(
                                "[S13.8] HDA codec topology: PASSED afg={} widgets={} dac={} adc={} mixers={} selectors={} pins={}",
                                topology.audio_function_groups,
                                topology.widgets,
                                topology.audio_outputs,
                                topology.audio_inputs,
                                topology.mixers,
                                topology.selectors,
                                topology.pin_complexes
                            ));
                        }
                        Err(error) => {
                            serial::write_line(format_args!(
                                "[S13.8] HDA codec topology: FAILED {:?}",
                                error
                            ));
                            qemu_test_exit_failure();
                        }
                    }
                    serial::write_line(format_args!("[S13.6] USB HID: FAILED report {:?}", error));
                        qemu_test_exit_failure();
                    }
                };
                serial::write_line(format_args!(
                    "[S13.6] USB HID keyboard: PASSED slot={} port={} interface={} endpoint={:#x} max_packet={} report={:02x?}",
                    summary.slot, summary.port, summary.interface, summary.endpoint, summary.max_packet, report
                ));
                qemu_test_exit_success();
            }
            Err(error) => {
                serial::write_line(format_args!("[S13.6] USB HID: FAILED init {:?}", error));
                qemu_test_exit_failure();
            }
        }
    }
    #[cfg(feature = "stage1-5-test")]
    { if !journal::structural_self_test() { serial::write_line(format_args!("[S1-5] storage journal: FAILED")); qemu_test_exit_failure(); } serial::write_line(format_args!("[S1-5] storage journal: PASSED")); qemu_test_exit_success(); }
    #[cfg(feature = "stage12-2-test")]
    { if !wovenfs::structural_self_test() { serial::write_line(format_args!("[S12.2] WovenFS: FAILED")); qemu_test_exit_failure(); } serial::write_line(format_args!("[S12.2] WovenFS: PASSED")); qemu_test_exit_success(); }
    #[cfg(feature = "stage12-3-test")]
    { if !volume_crypto::structural_self_test() { serial::write_line(format_args!("[S12.3] encryption: FAILED")); qemu_test_exit_failure(); } serial::write_line(format_args!("[S12.3] encryption: PASSED")); qemu_test_exit_success(); }
    #[cfg(feature = "stage12-4-test")]
    { if !snapshots::structural_self_test() { serial::write_line(format_args!("[S12.4] snapshots: FAILED")); qemu_test_exit_failure(); } serial::write_line(format_args!("[S12.4] snapshots: PASSED")); qemu_test_exit_success(); }
    #[cfg(feature = "stage12-5-test")]
    { if !storage_manager::structural_self_test() { serial::write_line(format_args!("[S12.5] storage management: FAILED")); qemu_test_exit_failure(); } serial::write_line(format_args!("[S12.5] storage management: PASSED")); qemu_test_exit_success(); }
    #[cfg(feature = "stage11-4-test")]
    {
        serial::write_line(format_args!("[S11.4] libwoven runtime boundary: PASSED"));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage11-5-test")]
    {
        if !userspace::elf_loader_self_test() {
            serial::write_line(format_args!("[S11.5] loader hardening: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S11.5] loader hardening: PASSED"));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage10-9-test")]
    {
        if !async_events::structural_self_test()
            || async_events::active_count() != 0
            || async_op::stats().active != 0
        {
            serial::write_line(format_args!("[S10.9] timers/events: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S10.9] timers/events/deadlines/cancellation/teardown: PASSED"));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage10-8-test")]
    {
        let before = completion_port::active_count();
        let program = userspace::create_stage10_8_process().expect("completion-port probe ELF");
        let Ok((pid, _)) = task::spawn_user_process("s10.8-ports", program) else {
            serial::write_line(format_args!("[S10.8] probe task creation: FAILED"));
            qemu_test_exit_failure();
        };
        let start = timer::ticks();
        while !task::process_exited(pid) {
            if timer::ticks().saturating_sub(start) > 500 {
                serial::write_line(format_args!("[S10.8] completion ports: TIMEOUT"));
                qemu_test_exit_failure();
            }
            x86_64::instructions::hlt();
        }
        if task::wait_process(pid.as_u64()) != Ok(0) || async_op::stats().active != 0
            || completion_port::active_count() != before || !async_acceptance::ports_smp()
        {
            serial::write_line(format_args!("[S10.8] completion ports: FAILED"));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S10.8] completion ports + batch/cancel/timeout/teardown/SMP: PASSED"));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage10-4-test")]
    {
        let block_before = block_io::stats();
        let async_before = async_op::stats();
        let Some(stage10_4_program) = userspace::create_stage10_4_process() else {
            serial::write_line(format_args!("[S10.4] userspace async block I/O image: FAILED"));
            qemu_test_exit_failure();
        };
        let stage10_4_caps = capability::CapabilitySet::only(capability::Capability::StorageIo);
        let stage10_4_pid = match task::spawn_user_system_service(
            "s10.4-async-block",
            stage10_4_program,
            stage10_4_caps,
        ) {
            Ok((pid, _, _)) => pid,
            Err(_) => {
                serial::write_line(format_args!("[S10.4] userspace async block I/O spawn: FAILED"));
                qemu_test_exit_failure();
            }
        };
        let stage10_4_start = timer::ticks();
        while !task::process_exited(stage10_4_pid) {
            if timer::ticks().wrapping_sub(stage10_4_start) > 500 {
                serial::write_line(format_args!("[S10.4] userspace async block I/O: TIMEOUT"));
                qemu_test_exit_failure();
            }
            x86_64::instructions::hlt();
        }
        if task::wait_process(stage10_4_pid.as_u64()) != Ok(0) {
            serial::write_line(format_args!("[S10.4] userspace async block I/O exit: FAILED"));
            qemu_test_exit_failure();
        }
        let block_after = block_io::stats();
        let async_after = async_op::stats();
        if block_after.queued < block_before.queued + 4
            || block_after.completed < block_before.completed + 2
            || block_after.active != 0
            || async_after.active != 0
            || async_after.owner_reaped <= async_before.owner_reaped
        {
            serial::write_line(format_args!(
                "[S10.4] userspace async block I/O: FAILED queued={} completed={} block_active={} async_active={} owner_reaped={}",
                block_after.queued.wrapping_sub(block_before.queued),
                block_after.completed.wrapping_sub(block_before.completed),
                block_after.active,
                async_after.active,
                async_after.owner_reaped.wrapping_sub(async_before.owner_reaped),
            ));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!(
            "[S10.4] userspace async block read/write + bounce-buffer teardown: PASSED"
        ));
        // Stage 10.4 is intentionally a dedicated boot mode. The complete
        // Stage 10.3 production validation has already run immediately before
        // these isolated boots, so terminate the dedicated QEMU probe here
        // rather than perturbing later legacy assertions with extra service
        // process/device activity.
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage10-5-test")]
    {
        if !async_file::start_worker() {
            serial::write_line(format_args!("[S10.5] async file worker start: FAILED"));
            qemu_test_exit_failure();
        }
        let async_before = async_op::stats();
        let file_before = async_file::stats();
        if vfs::write_file("/tmp/s10.5", &[0u8; 32]).is_err() {
            serial::write_line(format_args!("[S10.5] acceptance file creation: FAILED"));
            qemu_test_exit_failure();
        }
        let Some(program) = userspace::create_stage10_5_process() else {
            serial::write_line(format_args!("[S10.5] userspace async file image: FAILED"));
            qemu_test_exit_failure();
        };
        let pid = match task::spawn_user_process("s10.5-async-file", program) {
            Ok((pid, _)) => pid,
            Err(_) => {
                serial::write_line(format_args!("[S10.5] userspace async file spawn: FAILED"));
                qemu_test_exit_failure();
            }
        };
        let start = timer::ticks();
        while !task::process_exited(pid) {
            if timer::ticks().wrapping_sub(start) > 500 {
                serial::write_line(format_args!("[S10.5] userspace async file I/O: TIMEOUT"));
                qemu_test_exit_failure();
            }
            x86_64::instructions::hlt();
        }
        if task::wait_process(pid.as_u64()) != Ok(0) {
            serial::write_line(format_args!("[S10.5] userspace async file exit: FAILED"));
            qemu_test_exit_failure();
        }
        // Cancellation/exit may race a worker already holding a copied Work.
        // Wait only for deterministic kernel cleanup, not for userspace work.
        let cleanup_start = timer::ticks();
        while async_file::stats().active != 0 {
            if timer::ticks().wrapping_sub(cleanup_start) > 100 {
                serial::write_line(format_args!("[S10.5] async file teardown drain: TIMEOUT"));
                qemu_test_exit_failure();
            }
            task::yield_now();
        }
        let file_after = async_file::stats();
        let async_after = async_op::stats();
        let mut verify = [0u8; 16];
        let verified_bytes = vfs::open("/tmp/s10.5")
            .and_then(|id| {
                let result = vfs::read_at(id, 0, &mut verify);
                let _ = vfs::close_open_file(id);
                result
            })
            .is_ok_and(|count| count == 16)
            && verify == *b"WOVENHAT-ASYNC!!";
        let _ = vfs::remove("/tmp/s10.5");
        if file_after.submitted < file_before.submitted + 4
            || file_after.completed < file_before.completed + 2
            || file_after.cancelled <= file_before.cancelled
            || file_after.owner_reaped <= file_before.owner_reaped
            || file_after.active != 0
            || async_after.active != 0
            || async_after.owner_reaped <= async_before.owner_reaped
            || !verified_bytes
        {
            serial::write_line(format_args!(
                "[S10.5] userspace async VFS I/O: FAILED submitted={} completed={} cancelled={} file_reaped={} async_active={} generic_reaped={} bytes_ok={}",
                file_after.submitted.wrapping_sub(file_before.submitted),
                file_after.completed.wrapping_sub(file_before.completed),
                file_after.cancelled.wrapping_sub(file_before.cancelled),
                file_after.owner_reaped.wrapping_sub(file_before.owner_reaped),
                async_after.active,
                async_after.owner_reaped.wrapping_sub(async_before.owner_reaped),
                verified_bytes,
            ));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!(
            "[S10.5] userspace async VFS read/write + fd pinning/cancel/teardown: PASSED"
        ));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage10-6-test")]
    {
        if network::init().is_err() {
            serial::write_line(format_args!("[S10.6] virtio-net + smoltcp init: FAILED"));
            qemu_test_exit_failure();
        }
        if !async_network::start_worker() {
            serial::write_line(format_args!("[S10.6] async network worker start: FAILED"));
            qemu_test_exit_failure();
        }
        let async_before = async_op::stats();
        let net_before = async_network::stats();
        let sockets_before = network::stats().user_sockets;
        let Some(program) = userspace::create_stage10_6_process() else {
            serial::write_line(format_args!("[S10.6] userspace async network image: FAILED"));
            qemu_test_exit_failure();
        };
        let pid = match task::spawn_user_process("s10.6-async-net", program) {
            Ok((pid, _)) => pid,
            Err(_) => {
                serial::write_line(format_args!("[S10.6] userspace async network spawn: FAILED"));
                qemu_test_exit_failure();
            }
        };
        serial::write_line(format_args!("[S10.6] async UDP receive target ready on port 7001"));
        let start = timer::ticks();
        while !task::process_exited(pid) {
            network::poll();
            if timer::ticks().wrapping_sub(start) > 1_000 {
                serial::write_line(format_args!("[S10.6] userspace async network I/O: TIMEOUT"));
                qemu_test_exit_failure();
            }
            x86_64::instructions::hlt();
        }
        if task::wait_process(pid.as_u64()) != Ok(0) {
            serial::write_line(format_args!("[S10.6] userspace async network exit: FAILED"));
            qemu_test_exit_failure();
        }
        let cleanup_start = timer::ticks();
        while async_network::stats().active != 0 {
            network::poll();
            if timer::ticks().wrapping_sub(cleanup_start) > 200 {
                serial::write_line(format_args!("[S10.6] async network teardown drain: TIMEOUT"));
                qemu_test_exit_failure();
            }
            task::yield_now();
        }
        let net_after = async_network::stats();
        let async_after = async_op::stats();
        let sockets_after = network::stats().user_sockets;
        if net_after.submitted < net_before.submitted + 4
            || net_after.completed < net_before.completed + 2
            || net_after.cancelled <= net_before.cancelled
            || net_after.owner_reaped <= net_before.owner_reaped
            || net_after.active != 0
            || async_after.active != 0
            || async_after.owner_reaped <= async_before.owner_reaped
            || sockets_after != sockets_before
        {
            serial::write_line(format_args!(
                "[S10.6] userspace async networking: FAILED submitted={} completed={} cancelled={} net_reaped={} net_active={} async_active={} generic_reaped={} sockets_before={} sockets_after={}",
                net_after.submitted.wrapping_sub(net_before.submitted),
                net_after.completed.wrapping_sub(net_before.completed),
                net_after.cancelled.wrapping_sub(net_before.cancelled),
                net_after.owner_reaped.wrapping_sub(net_before.owner_reaped),
                net_after.active,
                async_after.active,
                async_after.owner_reaped.wrapping_sub(async_before.owner_reaped),
                sockets_before,
                sockets_after,
            ));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!(
            "[S10.6] userspace async UDP send/recv + socket pinning/cancel/teardown: PASSED"
        ));
        qemu_test_exit_success();
    }
    #[cfg(feature = "stage10-7-test")]
    {
        if network::init().is_err() {
            serial::write_line(format_args!("[S10.7] virtio-net + smoltcp init: FAILED"));
            qemu_test_exit_failure();
        }
        if !async_network::start_worker() {
            serial::write_line(format_args!("[S10.7] async network worker start: FAILED"));
            qemu_test_exit_failure();
        }
        let async_before = async_op::stats();
        let net_before = async_network::stats();
        let sockets_before = network::stats().user_sockets;
        let Some(program) = userspace::create_stage10_7_process() else {
            serial::write_line(format_args!("[S10.7] userspace async TCP image: FAILED"));
            qemu_test_exit_failure();
        };
        let pid = match task::spawn_user_process("s10.7-async-tcp", program) {
            Ok((pid, _)) => pid,
            Err(_) => {
                serial::write_line(format_args!("[S10.7] userspace async TCP spawn: FAILED"));
                qemu_test_exit_failure();
            }
        };
        serial::write_line(format_args!("[S10.7] async TCP host target 10.0.2.2:18080"));
        let start = timer::ticks();
        while !task::process_exited(pid) {
            network::poll();
            if timer::ticks().wrapping_sub(start) > 1_500 {
                serial::write_line(format_args!("[S10.7] userspace async TCP I/O: TIMEOUT"));
                qemu_test_exit_failure();
            }
            x86_64::instructions::hlt();
        }
        if task::wait_process(pid.as_u64()) != Ok(0) {
            serial::write_line(format_args!("[S10.7] userspace async TCP exit: FAILED"));
            qemu_test_exit_failure();
        }
        let cleanup_start = timer::ticks();
        while async_network::stats().active != 0 {
            network::poll();
            if timer::ticks().wrapping_sub(cleanup_start) > 200 {
                serial::write_line(format_args!("[S10.7] async TCP teardown drain: TIMEOUT"));
                qemu_test_exit_failure();
            }
            task::yield_now();
        }
        let net_after = async_network::stats();
        let async_after = async_op::stats();
        let sockets_after = network::stats().user_sockets;
        if net_after.submitted < net_before.submitted + 8
            || net_after.completed < net_before.completed + 6
            || net_after.cancelled <= net_before.cancelled
            || net_after.owner_reaped <= net_before.owner_reaped
            || net_after.active != 0
            || async_after.active != 0
            || async_after.owner_reaped <= async_before.owner_reaped
            || sockets_after != sockets_before
        {
            serial::write_line(format_args!("[S10.7] userspace async TCP: FAILED submitted={} completed={} cancelled={} net_reaped={} net_active={} async_active={} generic_reaped={} sockets_before={} sockets_after={}",
                net_after.submitted.wrapping_sub(net_before.submitted),
                net_after.completed.wrapping_sub(net_before.completed),
                net_after.cancelled.wrapping_sub(net_before.cancelled),
                net_after.owner_reaped.wrapping_sub(net_before.owner_reaped),
                net_after.active,
                async_after.active,
                async_after.owner_reaped.wrapping_sub(async_before.owner_reaped),
                sockets_before, sockets_after));
            qemu_test_exit_failure();
        }
        serial::write_line(format_args!("[S10.7] userspace async TCP connect/send/recv + EOF/pinning/cancel/teardown: PASSED"));
        qemu_test_exit_success();
    }
    if !syscall::user_identity_verified() {
        console.println("USER UID/GID SYSCALLS: FAILED");
        halt();
    }
    if !syscall::user_exec_verified() {
        console.println("USER EXEC SYSCALL: FAILED");
        halt();
    }
    console.println("USER EXEC ATOMIC REPLACEMENT: OK");
    if !syscall::user_fork_verified() {
        console.println("USER FORK SYSCALL: FAILED");
        halt();
    }
    console.println("USER FORK ADDRESS-SPACE CLONE: OK");
    if !syscall::user_standard_streams_verified() {
        console.println("USER STDIN SYSCALL: FAILED");
        halt();
    }
    console.println("USER STANDARD STREAMS: OK");
    console.println("USER UID/GID SYSCALLS: OK");
    serial::write_line(format_args!(
        "[BOOT] user mmap/write/read/munmap and frame return verified"
    ));

    if !syscall::user_io_verified()
        || task::open_file_count() != 0
        || vfs::node_count() != vfs_nodes_before_userspace
    {
        console.println("USER VFS/DESCRIPTOR SYSCALLS: FAILED");
        halt();
    }
    serial::write_line(format_args!(
        "[BOOT] user write/open/read/close and pointer validation verified"
    ));

    if !syscall::last_completed(syscall::Number::Getpid) {
        console.println("USER SYSCALL ABI: FAILED");
        halt();
    }
    if task::zombie_count() != 2 {
        console.println("PROCESS ZOMBIE RETENTION: FAILED");
        halt();
    }
    let first_status = syscall::invoke(syscall::Number::Waitpid, first_pid.as_u64(), 0, 0);
    let second_status = syscall::invoke(syscall::Number::Waitpid, second_pid.as_u64(), 0, 0);
    if first_status != 0 || second_status != 0 || task::zombie_count() != 0 {
        console.println("PROCESS WAIT/REAP: FAILED");
        halt();
    }
    if syscall::invoke(syscall::Number::Waitpid, first_pid.as_u64(), 0, 0) != u64::MAX {
        console.println("PROCESS DOUBLE-WAIT REJECTION: FAILED");
        halt();
    }
    serial::write_line(format_args!(
        "[BOOT] parent-child waitpid and zombie reaping verified"
    ));
    if memory::stats().allocated_frames != isolation_baseline || ipc::endpoint_count() != 0 {
        console.println("USER ADDRESS-SPACE RECLAMATION: FAILED");
        halt();
    }

    console.println("USER CR3 ISOLATION/W^X/RECLAMATION: OK");
    serial::write_line(format_args!(
        "[BOOT] isolated user CR3 roots {:#x} and {:#x} verified",
        first_root, second_root,
    ));
    serial::write_line(format_args!(
        "[BOOT] ring3 W^X mapping cleanup and frame reuse verified"
    ));
    console.println("RING3 PROCESS GETPID/EXIT: OK");
    serial::write_line(format_args!(
        "[BOOT] ring3 process and syscall ABI verified"
    ));
    // Exercise a real Ring-3 absent-page exception. The pager must park the
    // faulting context, populate the backing page on its worker task, then
    // resume the original instruction. mmaptest covers sparse lazy faults,
    // writes, fork, and kernel copy_to/from_user fault resolution.
    let pager_before = task::pager_stats();

    serial::write_line(format_args!("[PAGER-DIAG] install mmap test: BEGIN"));
    if !userspace::install_file_mmap_test() {
        console.println("PAGER MMAP IMAGE: INSTALL FAILED");
        halt();
    }
    serial::write_line(format_args!("[PAGER-DIAG] install mmap test: DONE"));

    let mut pager_image = alloc::vec![0u8; vfs::NODE_CAPACITY];
    serial::write_line(format_args!("[PAGER-DIAG] read /bin/mmaptest: BEGIN"));
    let Ok(pager_image_len) = vfs::read_all("/bin/mmaptest", &mut pager_image) else {
        console.println("PAGER MMAP IMAGE: READ FAILED");
        halt();
    };
    serial::write_line(format_args!(
        "[PAGER-DIAG] read /bin/mmaptest: DONE bytes={}",
        pager_image_len
    ));

    // Stage 7.2 termination/lifecycle smoke test. Keep the target Ready and
    // therefore provably off-CPU: the scheduler may retire it immediately,
    // but ProcessState::Exited must not be published until that retirement is
    // complete. wait_process then owns deferred address-space destruction.
    serial::write_line(format_args!("[TERM] create termination target: BEGIN"));
    let Some(kill_program) = userspace::create_shell_process() else {
        console.println("TERMINATION TEST IMAGE: LOAD FAILED");
        halt();
    };
    serial::write_line(format_args!("[TERM] create termination target: DONE"));
    serial::write_line(format_args!("[TERM] spawn termination target: BEGIN"));
    let kill_pid = match task::spawn_user_process("termination-target", kill_program) {
        Ok((pid, _)) => pid,
        Err(_) => {
            console.println("TERMINATION TEST PROCESS: SPAWN FAILED");
            halt();
        }
    };
    serial::write_line(format_args!("[TERM] spawn termination target: DONE pid={}", kill_pid.as_u64()));
    let exited_before_kill = task::process_exited(kill_pid);
    serial::write_line(format_args!("[TERM] process_exited before kill={}", exited_before_kill));
    let kill_result = if exited_before_kill {
        Err(())
    } else {
        task::kill_process(kill_pid.as_u64(), 15).map_err(|_| ())
    };
    serial::write_line(format_args!("[TERM] kill result={}", kill_result.is_ok()));
    let termination_deadline = timer::ticks().wrapping_add(200);
    while !task::process_exited(kill_pid) && timer::ticks() < termination_deadline {
        task::yield_now();
    }
    let exited_after_kill = task::process_exited(kill_pid);
    serial::write_line(format_args!("[TERM] process_exited after kill={}", exited_after_kill));
    let wait_status = task::wait_process(kill_pid.as_u64());
    let wait_ok = wait_status == Ok(143);
    match wait_status {
        Ok(code) => serial::write_line(format_args!("[TERM] wait status code={}", code)),
        Err(task::WaitError::NoSuchChild) => {
            serial::write_line(format_args!("[TERM] wait status=NO_SUCH_CHILD"))
        }
        Err(task::WaitError::StillRunning) => {
            serial::write_line(format_args!("[TERM] wait status=STILL_RUNNING"))
        }
    }
    if exited_before_kill || kill_result.is_err() || !exited_after_kill || !wait_ok {
        console.println("SCHEDULER-OWNED TERMINATION: FAILED");
        halt();
    }
    serial::write_line(format_args!(
        "[TERM] scheduler-owned Ready-task termination + deferred reap: PASSED"
    ));

    serial::write_line(format_args!("[PAGER-DIAG] load mmap ELF: BEGIN"));
    let Some(pager_program) =
        userspace::load_elf_with_argv(&pager_image[..pager_image_len], &["/bin/mmaptest"])
    else {
        console.println("PAGER MMAP IMAGE: LOAD FAILED");
        halt();
    };
    serial::write_line(format_args!("[PAGER-DIAG] load mmap ELF: DONE"));

    let fork_affinity_before = task::fork_affinity_inheritances();

    // Stage 7.5: execute the existing file-backed mmap/pager workload on an
    // application processor when SMP is available. The task stays hard-pinned
    // for its lifetime, so this milestone validates remote Ring-3 file faults,
    // BSP pager wakeup, address-space resume and per-CPU I/O preemption guards
    // without yet enabling unrestricted automatic userspace balancing.
    let pager_cpu = if smp::online_count() > 1 {
        smp::online_count() - 1
    } else {
        0
    };
    serial::write_line(format_args!(
        "[PAGER-DIAG] spawn pager process: BEGIN cpu={}",
        pager_cpu
    ));
    let pager_pid = if pager_cpu == 0 {
        match task::spawn_user_process("pager-mmaptest", pager_program) {
            Ok((pid, _)) => pid,
            Err(_) => {
                console.println("PAGER MMAP PROCESS: SPAWN FAILED");
                halt();
            }
        }
    } else {
        match task::spawn_multicore_user_process(
            "pager-mmaptest",
            pager_program,
            1usize << pager_cpu,
        ) {
            Ok((pid, _, _, placement)) if placement.owner_cpu == pager_cpu => pid,
            Ok(_) => {
                console.println("PAGER MMAP PROCESS: AP OWNERSHIP FAILED");
                halt();
            }
            Err(_) => {
                console.println("PAGER MMAP PROCESS: AP SPAWN FAILED");
                halt();
            }
        }
    };
    serial::write_line(format_args!(
        "[PAGER-DIAG] spawn pager process: DONE pid={}",
        pager_pid.as_u64()
    ));
    serial::write_line(format_args!(
        "[PAGER-DIAG] first process_exited lookup: BEGIN pid={}",
        pager_pid.as_u64()
    ));
    let mut pager_exited = task::process_exited(pager_pid);
    serial::write_line(format_args!(
        "[PAGER-DIAG] first process_exited lookup: DONE exited={}",
        pager_exited
    ));

    // Stage 7.1.5 keeps the detailed hand-off tracer available for targeted
    // debugging, but normal acceptance runs it disabled so timing is not
    // distorted by per-switch serial I/O.
    task::set_yield_trace_enabled(false);
    let pager_wait_start = timer::ticks();
    let mut pager_wait_iteration = 0u64;
    while !pager_exited {
        if timer::ticks().wrapping_sub(pager_wait_start) > 200 {
            let (queued, completed) = task::pager_stats();
            serial::write_line(format_args!(
                "[PAGER] mmaptest timeout queued={} completed={}",
                queued, completed
            ));
            console.println("ASYNCHRONOUS PAGER: TIMEOUT");
            halt();
        }

        // This kernel-side acceptance loop must explicitly yield to the
        // Ready Ring-3 pager test process.  On SMP, merely executing HLT
        // waits for an interrupt but does not guarantee that the current
        // high-priority kernel task gives the scheduler a deterministic
        // opportunity to run the process whose page faults have already
        // been serviced.
        pager_wait_iteration = pager_wait_iteration.wrapping_add(1);
        if pager_wait_iteration <= 8 || pager_wait_iteration.is_multiple_of(64) {
            serial::write_line(format_args!(
                "[PAGER-DIAG] yield: BEGIN iteration={}",
                pager_wait_iteration
            ));
        }

        task::yield_now();

        if pager_wait_iteration <= 8 || pager_wait_iteration.is_multiple_of(64) {
            serial::write_line(format_args!(
                "[PAGER-DIAG] yield: DONE iteration={}",
                pager_wait_iteration
            ));
            serial::write_line(format_args!(
                "[PAGER-DIAG] process_exited lookup: BEGIN iteration={}",
                pager_wait_iteration
            ));
        }

        pager_exited = task::process_exited(pager_pid);

        if pager_wait_iteration <= 8 || pager_wait_iteration.is_multiple_of(64) {
            serial::write_line(format_args!(
                "[PAGER-DIAG] process_exited lookup: DONE iteration={} exited={}",
                pager_wait_iteration,
                pager_exited
            ));
        }
    }
    task::set_yield_trace_enabled(false);
    let pager_status = task::wait_process(pager_pid.as_u64());
    let pager_after = task::pager_stats();
    if pager_status != Ok(0) || pager_after.0 <= pager_before.0 || pager_after.1 < pager_after.0 {
        console.println("ASYNCHRONOUS PAGER: FAILED");
        halt();
    }
    serial::write_line(format_args!(
        "[PAGER] Ring-3 faults queued={} completed={}: PASSED",
        pager_after.0 - pager_before.0,
        pager_after.1 - pager_before.1
    ));
    if pager_cpu == 0 {
        serial::write_line(format_args!(
            "[S7.5] single-CPU pager/file-I/O baseline preserved"
        ));
    } else {
        serial::write_line(format_args!(
            "[S7.5] pinned AP pager/file-I/O: PASSED cpu={}",
            pager_cpu
        ));
    }
    if pager_cpu != 0 {
        if task::fork_affinity_inheritances() <= fork_affinity_before {
            serial::write_line(format_args!(
                "[S7.6] AP fork CPU/affinity inheritance: FAILED"
            ));
            halt();
        }
        serial::write_line(format_args!(
            "[S7.6] AP fork CPU/affinity inheritance: PASSED cpu={}",
            pager_cpu
        ));
        serial::write_line(format_args!(
            "[S7.6] bounded integrated multicore userspace foundation: PASSED"
        ));
    } else {
        serial::write_line(format_args!(
            "[S7.6] fork-affinity baseline preserved on single CPU"
        ));
    }
    console.println("KEYBOARD IRQ: READY");

    //
    // Breakpoint exception test
    //

    console.println("TESTING BREAKPOINT INTERRUPT...");

    int3();

    if interrupts::breakpoint_reached() {
        console.println("BREAKPOINT HANDLER: OK");
        console.println("INTERRUPT SYSTEM: ONLINE");
    } else {
        console.println("BREAKPOINT HANDLER: FAILED");
    }

    smp::self_test();
    serial::write_line(format_args!("[BOOT] ALL VALIDATIONS PASSED"));
    #[cfg(feature = "qemu-test")]
    qemu_test_exit_success();

    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(feature = "qemu-test")]
fn qemu_test_exit_success() {
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xf4_u16,
            in("eax") 0x10_u32,
            options(nomem, nostack, preserves_flags),
        );
    }
    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(feature = "qemu-test")]
fn qemu_test_exit_failure() -> ! {
    // QEMU isa-debug-exit maps value 0x11 to host exit status 35. This keeps
    // acceptance failures fail-fast instead of parking in halt() until the
    // host-side 180-second timeout expires.
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xf4_u16,
            in("eax") 0x11_u32,
            options(nomem, nostack, preserves_flags),
        );
    }
    loop {
        x86_64::instructions::hlt();
    }
}

fn fairness_probe_a() -> ! {
    while FAIR_TASK_B_RUNS.load(Ordering::Acquire) == 0 {
        FAIR_TASK_A_RUNS.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
    FAIR_TASK_A_RUNS.fetch_add(1, Ordering::Release);
    FAIR_TASKS_COMPLETED.fetch_add(1, Ordering::Release);
    task::exit_current_task()
}

fn fairness_probe_b() -> ! {
    while FAIR_TASK_A_RUNS.load(Ordering::Acquire) == 0 {
        FAIR_TASK_B_RUNS.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
    FAIR_TASK_B_RUNS.fetch_add(1, Ordering::Release);
    FAIR_TASKS_COMPLETED.fetch_add(1, Ordering::Release);
    task::exit_current_task()
}
fn preemption_probe_task() -> ! {
    task::sleep_current(2);
    PREEMPTION_PROBE_BLOCKED.store(true, Ordering::Release);
    task::block_current();
    PREEMPTION_PROBE_COMPLETED.store(true, Ordering::Release);
    task::exit_current_task()
}

fn halt() -> ! {
    x86_64::instructions::interrupts::disable();
    loop {
        x86_64::instructions::hlt();
    }
}

#[alloc_error_handler]
fn alloc_error_handler(layout: Layout) -> ! {
    serial::write_fmt(format_args!(
        "\nKERNEL ALLOC ERROR: layout size={} align={}\n",
        layout.size(),
        layout.align()
    ));
    halt()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    panic::kernel_panic(info)
}
