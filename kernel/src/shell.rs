//! Kernel debug shell — interactive console attached to the boot task.
//!
//! This is not a userspace shell. It runs with kernel privileges (subject to
//! the current task's capability set) and operates directly on the VFS,
//! scheduler, and hardware status helpers.

use crate::irq_lock::{IrqMutex, IrqMutexGuard};
use crate::{
    benchmark, block_io, capability::Capability, console::Console, device, heap, keyboard::Key,
    memory, network, paging, storage, swap, syscall, task, terminal, timer, userspace, vfs,
    virtio_net,
};

const PROMPT_PREFIX: &str = "wovenhat:";
const COMMAND_CAPACITY: usize = 128;
const CWD_CAPACITY: usize = crate::config::MAX_PATH_SIZE;

/// Kernel shell working directory (independent of userspace process cwd).
struct ShellState {
    cwd: [u8; CWD_CAPACITY],
    cwd_len: usize,
}

impl ShellState {
    const fn new() -> Self {
        let mut cwd = [0u8; CWD_CAPACITY];
        cwd[0] = b'/';
        Self { cwd, cwd_len: 1 }
    }

    fn cwd_str(&self) -> &str {
        core::str::from_utf8(&self.cwd[..self.cwd_len]).unwrap_or("/")
    }

    fn set_cwd(&mut self, path: &str) -> bool {
        let bytes = path.as_bytes();
        if bytes.is_empty() || bytes.len() >= CWD_CAPACITY {
            return false;
        }
        self.cwd = [0; CWD_CAPACITY];
        self.cwd[..bytes.len()].copy_from_slice(bytes);
        self.cwd_len = bytes.len();
        true
    }
}

static STATE: IrqMutex<ShellState> = IrqMutex::with_rank(ShellState::new(), 10);

fn state() -> IrqMutexGuard<'static, ShellState> {
    STATE.lock()
}

fn cwd_owned() -> alloc::string::String {
    let mut bytes = [0u8; CWD_CAPACITY];
    let length = {
        let state = state();
        let cwd = state.cwd_str().as_bytes();
        bytes[..cwd.len()].copy_from_slice(cwd);
        cwd.len()
    };
    alloc::string::String::from(core::str::from_utf8(&bytes[..length]).unwrap_or("/"))
}

pub struct Shell {
    command: [u8; COMMAND_CAPACITY],
    length: usize,
}

impl Shell {
    pub const fn new() -> Self {
        Self {
            command: [0; COMMAND_CAPACITY],
            length: 0,
        }
    }

    pub fn print_prompt(&self, console: &mut Console<'_>) {
        console.print(PROMPT_PREFIX);
        let cwd = cwd_owned();
        console.print(&cwd);
        console.print("> ");
    }

    pub fn handle_key(&mut self, key: Key, console: &mut Console<'_>) {
        match key {
            Key::Char(character) => self.push_character(character, console),
            Key::Backspace => self.backspace(console),
            Key::Enter => self.submit(console),
            Key::Tab | Key::F1 => {}
        }
    }

    fn push_character(&mut self, character: char, console: &mut Console<'_>) {
        if !character.is_ascii() || character.is_ascii_control() {
            return;
        }
        if self.length == self.command.len() {
            return;
        }
        self.command[self.length] = character as u8;
        self.length += 1;
        console.put_char(character);
    }

    fn backspace(&mut self, console: &mut Console<'_>) {
        if self.length == 0 {
            return;
        }
        self.length -= 1;
        console.backspace();
    }

    fn submit(&mut self, console: &mut Console<'_>) {
        console.newline();

        let line = core::str::from_utf8(&self.command[..self.length])
            .unwrap_or("")
            .trim();
        let (verb, arg) = split_command(line);
        // Accept both lower and UPPER for convenience at the serial console.
        let mut verb_buf = [0u8; 32];
        let verb = lowercase(verb, &mut verb_buf);

        match verb {
            "" => {}
            "help" | "?" => print_help(console),
            "clear" => {
                if authorize(Capability::Console, console) {
                    console.clear();
                }
            }
            "version" | "ver" => console.println("WovenHat kernel 0.8.0 Multicore Foundation"),
            "ticks" | "uptime" => {
                if authorize(Capability::TimerRead, console) {
                    console.print("ticks: ");
                    print_u64(console, timer::ticks());
                    console.newline();
                }
            }
            "tasks" | "ps" => {
                if authorize(Capability::TaskInspect, console) {
                    cmd_tasks(console);
                }
            }
            "smp" => {
                if authorize(Capability::TaskInspect, console) {
                    crate::smp::diagnostic(console);
                }
            }
            "smptest" => {
                if authorize(Capability::TaskControl, console) {
                    crate::smp::self_test();
                    console.println("SMP regressions: PASSED");
                }
            }
            "caps" => cmd_caps(console),
            "devices" | "dev" => cmd_devices(console),
            "blockio" | "iostat" => {
                if authorize(Capability::TaskInspect, console) {
                    cmd_block_io(console);
                }
            }
            "net" => cmd_net(console),
            "netstat" => cmd_netstat(console),
            "udpecho" => cmd_udpecho(arg, console),
            "dhcp" => cmd_dhcp(arg, console),
            "userland" | "bin" => {
                if authorize(Capability::FileWrite, console) {
                    cmd_userland(console);
                }
            }
            "kill" => {
                if authorize(Capability::TaskControl, console) {
                    cmd_kill(arg, console);
                }
            }
            "memory" | "mem" => {
                if authorize(Capability::MemoryInspect, console) {
                    cmd_memory(console);
                }
            }
            "heap" => {
                if authorize(Capability::MemoryInspect, console) {
                    cmd_heap(console);
                }
            }
            "paging" => {
                if authorize(Capability::MemoryInspect, console) {
                    cmd_paging(console);
                }
            }
            "bench" => {
                if authorize(Capability::TaskInspect, console)
                    && authorize(Capability::MemoryInspect, console)
                {
                    cmd_bench(console);
                }
            }
            "fs" => {
                if authorize(Capability::FileRead, console) {
                    cmd_fs(console);
                }
            }
            "ls" => {
                if authorize(Capability::FileRead, console) {
                    let path_buf;
                    let path = if arg.is_empty() {
                        path_buf = cwd_owned();
                        path_buf.as_str()
                    } else {
                        arg
                    };
                    cmd_ls(path, console);
                }
            }
            "cat" => {
                if authorize(Capability::FileRead, console) {
                    if arg.is_empty() {
                        console.println("usage: cat <path>");
                    } else {
                        cmd_cat(arg, console);
                    }
                }
            }
            "write" => {
                if authorize(Capability::FileWrite, console) {
                    cmd_write(arg, console);
                }
            }
            "rm" | "remove" => {
                if authorize(Capability::FileWrite, console) {
                    if arg.is_empty() {
                        console.println("usage: rm <path>");
                    } else {
                        cmd_rm(arg, console);
                    }
                }
            }
            "rename" | "mv" => {
                if authorize(Capability::FileWrite, console) {
                    cmd_rename(arg, console);
                }
            }
            "mkdir" => {
                if authorize(Capability::FileWrite, console) {
                    if arg.is_empty() {
                        console.println("usage: mkdir <path>");
                    } else {
                        cmd_mkdir(arg, console);
                    }
                }
            }
            "stat" => {
                if authorize(Capability::FileRead, console) {
                    if arg.is_empty() {
                        console.println("usage: stat <path>");
                    } else {
                        cmd_stat(arg, console);
                    }
                }
            }
            "echo" => {
                if arg.is_empty() {
                    console.newline();
                } else {
                    console.println(arg);
                }
            }
            "cd" => {
                if authorize(Capability::FileRead, console) {
                    let path = if arg.is_empty() { "/" } else { arg };
                    if !shell_chdir(path) {
                        console.println("cd: failed");
                    }
                }
            }
            "pwd" => {
                if authorize(Capability::FileRead, console) {
                    let cwd = cwd_owned();
                    console.println(&cwd);
                }
            }
            "msynctest" => {
                if authorize(Capability::FileRead, console)
                    && authorize(Capability::FileWrite, console)
                {
                    let passed = userspace::shared_disk_mmap_self_test()
                        && userspace::disk_unlink_mmap_self_test();
                    console.println(if passed {
                        "MSYNC DISK: PASS"
                    } else {
                        "MSYNC DISK: FAIL (check ATA disk or existing /mnt/vmsync.txt)"
                    });
                    crate::serial::write_line(format_args!(
                        "[MSYNC DISK] {}",
                        if passed { "PASSED" } else { "FAILED" }
                    ));
                }
            }
            "mmaptest" => {
                if authorize(Capability::FileRead, console)
                    && authorize(Capability::TaskControl, console)
                    && authorize(Capability::ProcessCreate, console)
                {
                    if ensure_program("/bin/mmaptest", userspace::install_file_mmap_test) {
                        cmd_run("/bin/mmaptest", console, true);
                    } else {
                        console.println("mmaptest: install failed");
                    }
                }
            }
            "run" => {
                if authorize(Capability::TaskControl, console)
                    && authorize(Capability::ProcessCreate, console)
                {
                    if arg.is_empty() {
                        console.println("usage: run <path>");
                    } else {
                        cmd_run(arg, console, false);
                    }
                }
            }
            "sh" => {
                if authorize(Capability::TaskControl, console) {
                    cmd_sh(console);
                }
            }
            "init" => {
                if authorize(Capability::TaskControl, console) {
                    cmd_init(console);
                }
            }
            "spawn" => {
                if authorize(Capability::TaskControl, console) {
                    match task::spawn("demo", demo_task) {
                        Ok(id) => {
                            console.print("task spawned: ");
                            print_u64(console, id.as_u64());
                            console.newline();
                        }
                        Err(_) => console.println("spawn failed: scheduler full"),
                    }
                }
            }
            "syscall" => {
                if authorize(Capability::InterruptControl, console) {
                    console.println("triggering syscall 0x80");
                    if syscall::test() {
                        console.println("syscall handler: ok");
                    } else {
                        console.println("syscall handler: failed");
                    }
                }
            }
            "user" | "ring3" => {
                if authorize(Capability::TaskControl, console) {
                    cmd_user(console);
                }
            }
            "mount" => {
                if authorize(Capability::FileWrite, console) {
                    cmd_mount(arg, console);
                }
            }
            "remount" | "rescan" => {
                if authorize(Capability::FileWrite, console) {
                    cmd_mount("/mnt", console);
                }
            }
            "umount" | "unmount" => {
                if authorize(Capability::FileWrite, console) {
                    cmd_umount(arg, console);
                }
            }
            "df" => {
                if authorize(Capability::FileRead, console) {
                    cmd_df(arg, console);
                }
            }
            "fscheck" => {
                if authorize(Capability::FileRead, console) {
                    cmd_fscheck(arg, console);
                }
            }
            "persist" => {
                if authorize(Capability::FileWrite, console) {
                    cmd_persist(arg, console);
                }
            }
            "sync" => {
                if authorize(Capability::FileWrite, console) {
                    cmd_sync(console);
                }
            }
            _ => {
                if !cmd_userland_command(verb, arg, console) {
                    console.print("unknown command: ");
                    console.println(verb);
                    console.println("type 'help' for a list");
                }
            }
        }

        self.finish(console);
    }

    fn finish(&mut self, console: &mut Console<'_>) {
        self.length = 0;
        if !terminal::foreground_active() {
            self.print_prompt(console);
        }
    }
}

fn print_help(console: &mut Console<'_>) {
    console.println("WovenHat kernel shell 0.8.0 Multicore Foundation");
    console.println(
        "system:  help clear version ticks|uptime tasks|ps smp smptest caps devices net netstat",
    );
    console.println(
        "         memory|mem heap paging bench fs blockio|iostat mount|umount df fscheck sync syscall",
    );
    console.println("files:   ls [path]  cat <path>  write <path> <text>");
    console.println("         mkdir <path>  rm <path>  stat <path>");
    console.println("         rename|mv <old> <new>");
    console.println("test:    mmaptest (private/shared mappings, fork, msync), msynctest (disk)");
    console.println("nav:     cd [path]  pwd  echo <text>");
    console.println("process: run <elf>  sh  init  spawn  user|ring3  kill <pid> [sig]");
    console
        .println("runtime: userland udpecho [port] dhcp <on|off>   (Multicore Foundation runtime)");
}

fn cmd_tasks(console: &mut Console<'_>) {
    let summary = task::summary();
    console.print("tasks: ");
    print_u64(console, summary.task_count as u64);
    console.print("  processes: ");
    print_u64(console, task::process_count() as u64);
    console.print("  ready: ");
    print_u64(console, summary.ready_tasks as u64);
    console.print("  blocked: ");
    print_u64(console, summary.blocked_tasks as u64);
    console.newline();
    console.print("current: ");
    print_u64(console, summary.current_id.as_u64());
    console.print(" ");
    console.print(summary.current_name);
    console.print("  state: ");
    console.print(summary.current_state);
    console.print("  priority: ");
    print_u64(console, summary.current_priority as u64);
    console.newline();
    console.print("switches: ");
    print_u64(console, summary.context_switches);
    console.print("  preemptions: ");
    print_u64(console, summary.preemption_switches);
    console.print("  idle: ");
    print_u64(console, summary.idle_heartbeats);
    console.newline();
}

fn cmd_devices(console: &mut Console<'_>) {
    console.print("devices: ");
    print_u64(console, device::count() as u64);
    console.newline();

    for name in [
        "framebuffer-console",
        "com1",
        "pit",
        "ps2-keyboard",
        "ata0",
        "hda0",
    ] {
        if let Some(dev) = device::find(name) {
            console.print("  ");
            console.print(dev.name);
            console.print("  kind=");
            console.print(match dev.kind {
                device::DeviceKind::Console => "console",
                device::DeviceKind::Serial => "serial",
                device::DeviceKind::Timer => "timer",
                device::DeviceKind::Keyboard => "keyboard",
                device::DeviceKind::Block => "block",
                device::DeviceKind::Audio => "audio",
            });
            if let Some(irq) = dev.irq {
                console.print(" irq=");
                print_u64(console, irq as u64);
            }
            console.newline();
        }
    }
}

fn cmd_net(console: &mut Console<'_>) {
    match virtio_net::probe() {
        virtio_net::ProbeStatus::Found(loc) => {
            console.print("virtio-net pci: ");
            print_u64(console, loc.bus as u64);
            console.print(":");
            print_u64(console, loc.device as u64);
            console.print(".");
            print_u64(console, loc.function as u64);
            console.newline();
        }
        virtio_net::ProbeStatus::Missing => {
            console.println("virtio-net: not present");
            console.println("QEMU requires transitional device: disable-modern=on");
            return;
        }
    }

    let stats = virtio_net::stats();
    console.print("transport: ");
    console.println(if stats.initialized {
        "legacy virtqueue DMA online"
    } else {
        "not initialized"
    });
    if !stats.initialized {
        console.println("QEMU: -device virtio-net-pci,netdev=net0,disable-modern=on");
        console.println("      -netdev user,id=net0");
        return;
    }
    console.print("smoltcp: ");
    console.println(if network::initialized() {
        "online"
    } else {
        "offline"
    });
    let info = network::net_info();
    console.print("ipv4: ");
    print_ipv4(console, info.ipv4);
    console.print("/");
    print_u64(console, info.prefix as u64);
    console.print("  gateway: ");
    print_ipv4(console, info.gateway);
    console.print("  dns: ");
    print_ipv4(console, info.dns);
    console.print("  config: ");
    console.println(if info.using_dhcp != 0 {
        "dhcp"
    } else {
        "static-fallback"
    });
    console.print("rx=");
    print_u64(console, stats.rx_frames);
    console.print(" tx=");
    print_u64(console, stats.tx_frames);
    console.print(" rx_drop=");
    print_u64(console, stats.rx_dropped);
    console.print(" tx_busy=");
    print_u64(console, stats.tx_busy);
    console.newline();
}

fn cmd_netstat(console: &mut Console<'_>) {
    let stats = network::stats();
    console.print("network: ");
    console.println(if stats.online { "online" } else { "offline" });
    console.print("udp echo: ");
    if stats.echo_active {
        console.print("listening port=");
        print_u64(console, stats.echo_port as u64);
        console.print(" packets=");
        print_u64(console, stats.echo_packets);
        console.newline();
    } else {
        console.println("stopped");
    }
    console.print("userspace sockets: ");
    print_u64(console, stats.user_sockets as u64);
    console.newline();
    console.print("dhcp: ");
    if stats.dhcp_enabled {
        console.println(if stats.using_dhcp {
            "lease active"
        } else {
            "discovering / static fallback"
        });
    } else {
        console.println("disabled (static fallback)");
    }
}

fn cmd_udpecho(arg: &str, console: &mut Console<'_>) {
    let port = if arg.trim().is_empty() {
        7
    } else {
        match arg.trim().parse::<u16>() {
            Ok(port) if port != 0 => port,
            _ => {
                console.println("usage: udpecho [1..65535]");
                return;
            }
        }
    };
    match network::start_udp_echo(port) {
        Ok(()) => {
            console.print("udp echo listening on 10.0.2.15:");
            print_u64(console, port as u64);
            console.newline();
        }
        Err(network::EchoError::NetworkOffline) => console.println("udpecho: network offline"),
        Err(network::EchoError::AlreadyConfigured) => {
            console.println("udpecho: already listening on another port")
        }
        Err(_) => console.println("udpecho: start failed"),
    }
}

fn cmd_dhcp(arg: &str, console: &mut Console<'_>) {
    match arg.trim() {
        "on" | "enable" | "1" => match network::set_dhcp(true) {
            Ok(()) => console
                .println("dhcp: enabled; static configuration remains until a lease is acquired"),
            Err(_) => console.println("dhcp: network offline"),
        },
        "off" | "disable" | "0" => match network::set_dhcp(false) {
            Ok(()) => console.println("dhcp: disabled; using static QEMU fallback"),
            Err(_) => console.println("dhcp: network offline"),
        },
        _ => console.println("usage: dhcp <on|off>"),
    }
}

fn print_ipv4(console: &mut Console<'_>, ip: [u8; 4]) {
    print_u64(console, ip[0] as u64);
    console.print(".");
    print_u64(console, ip[1] as u64);
    console.print(".");
    print_u64(console, ip[2] as u64);
    console.print(".");
    print_u64(console, ip[3] as u64);
}

fn cmd_persist(arg: &str, console: &mut Console<'_>) {
    let arg = arg.trim();
    if arg.is_empty() {
        console.println("usage: persist </mnt/path>");
        return;
    }
    let Some(path) = shell_resolve(arg) else {
        console.println("persist: bad path");
        return;
    };
    if touches_mnt(&path) && !storage::mnt_mounted() {
        console.println("persist: /mnt is not mounted");
        return;
    }
    let result = match vfs::stat(&path) {
        Ok(stat) if stat.kind == vfs::NodeKind::Directory => storage::persist_directory(&path),
        Ok(_) => storage::persist_path(&path),
        Err(_) => {
            console.println("persist: not found");
            return;
        }
    };
    match result {
        Ok(()) => console.println("persist: written to FAT32"),
        Err(storage::PersistError::Unmounted) => console.println("persist: /mnt is not mounted"),
        Err(storage::PersistError::NotSupported) => {
            console.println("persist: path must be under /mnt")
        }
        Err(storage::PersistError::NoDevice) => console.println("persist: no ATA disk"),
        Err(storage::PersistError::BadName) => console.println("persist: FAT 8.3 path required"),
        Err(storage::PersistError::TooLarge) => {
            console.println("persist: no space or file too large")
        }
        Err(_) => console.println("persist: write failed"),
    }
}

fn cmd_sync(console: &mut Console<'_>) {
    match storage::sync_all_mounted() {
        Ok(count) => {
            console.print("sync: persisted ");
            print_u64(console, count as u64);
            console.println(" file(s) under /mnt");
        }
        Err(storage::PersistError::Unmounted) => console.println("sync: /mnt is not mounted"),
        Err(storage::PersistError::NoDevice) => console.println("sync: no ATA disk"),
        Err(_) => console.println("sync: failed; dirty buffers retained for retry"),
    }
}
fn cmd_block_io(console: &mut Console<'_>) {
    let stats = block_io::stats();
    console.print("block io: queued=");
    print_u64(console, stats.queued);
    console.print(" completed=");
    print_u64(console, stats.completed);
    console.print(" direct=");
    print_u64(console, stats.direct);
    console.print(" pending=");
    print_u64(console, stats.pending as u64);
    console.print(" active=");
    print_u64(console, stats.active as u64);
    console.newline();

    let swap_stats = swap::stats();
    console.print("swap: used=");
    print_u64(console, swap_stats.used as u64);
    console.print(" ram=");
    print_u64(console, swap_stats.ram_used as u64);
    console.print(" disk=");
    print_u64(console, swap_stats.disk_used as u64);
    console.print(" disk_slots=");
    print_u64(console, swap_stats.disk_slots as u64);
    console.newline();
}

fn ensure_program(path: &str, installer: fn() -> bool) -> bool {
    vfs::stat(path).is_ok() || installer()
}

fn install_userland() -> u64 {
    type Installer = (&'static str, fn() -> bool);
    let programs: [Installer; 23] = [
        ("/bin/selftest", userspace::install_stub_executable),
        ("/bin/init", userspace::install_init_executable),
        ("/bin/sh", userspace::install_shell_executable),
        ("/bin/echo", userspace::install_echo_executable),
        ("/bin/true", userspace::install_true_executable),
        ("/bin/false", userspace::install_false_executable),
        ("/bin/cat", userspace::install_cat_executable),
        ("/bin/ls", userspace::install_ls_executable),
        ("/bin/sleep", userspace::install_sleep_executable),
        ("/bin/pwd", userspace::install_pwd_executable),
        ("/bin/mkdir", userspace::install_mkdir_executable),
        ("/bin/ip", userspace::install_ip_executable),
        ("/bin/netstat", userspace::install_netstat_executable),
        ("/bin/dns", userspace::install_dns_executable),
        ("/bin/udp", userspace::install_udp_executable),
        ("/bin/nc", userspace::install_nc_executable),
        ("/bin/ping", userspace::install_ping_executable),
        ("/bin/env", userspace::install_env_executable),
        ("/bin/en", userspace::install_en_executable),
        ("/bin/bin", userspace::install_bin_executable),
        ("/bin/ps", userspace::install_ps_executable),
        ("/bin/uptime", userspace::install_uptime_executable),
        ("/bin/tcpd", userspace::install_tcpd_executable),
    ];

    let mut installed = 0u64;
    for (path, installer) in programs {
        if ensure_program(path, installer) {
            installed += 1;
        }
    }
    if ensure_program("/bin/rm", userspace::install_rm_executable) {
        installed += 1;
    }
    installed
}

fn cmd_userland(console: &mut Console<'_>) {
    let installed = install_userland();
    console.print("userland: ");
    print_u64(console, installed);
    console.println("/24 programs ready");
    if installed == 24 {
        console.println("/bin is ready; type 'sh' for the userspace shell");
    } else {
        console.println("userland: one or more built-in programs failed to install");
        const EXPECTED: [&str; 24] = [
            "/bin/selftest",
            "/bin/init",
            "/bin/sh",
            "/bin/echo",
            "/bin/true",
            "/bin/false",
            "/bin/cat",
            "/bin/ls",
            "/bin/sleep",
            "/bin/pwd",
            "/bin/mkdir",
            "/bin/rm",
            "/bin/ip",
            "/bin/netstat",
            "/bin/dns",
            "/bin/udp",
            "/bin/nc",
            "/bin/ping",
            "/bin/env",
            "/bin/en",
            "/bin/ps",
            "/bin/uptime",
            "/bin/tcpd",
            "/bin/bin",
        ];
        for path in EXPECTED {
            if vfs::stat(path).is_err() {
                console.print("missing: ");
                console.println(path);
            }
        }
    }
}

fn cmd_kill(arg: &str, console: &mut Console<'_>) {
    let (pid_text, signal_text) = split_command(arg);
    if pid_text.is_empty() {
        console.println("usage: kill <pid> [signal]");
        return;
    }
    let Ok(pid) = pid_text.parse::<u64>() else {
        console.println("kill: invalid pid");
        return;
    };
    let signal = if signal_text.is_empty() {
        15
    } else {
        match signal_text.parse::<u64>() {
            Ok(value) if value <= 31 => value,
            _ => {
                console.println("kill: invalid signal (0..31)");
                return;
            }
        }
    };
    match task::kill_process(pid, signal) {
        Ok(()) => console.println("kill: signal delivered"),
        Err(_) => console.println("kill: process not found"),
    }
}

fn cmd_caps(console: &mut Console<'_>) {
    console.print("caps:");
    print_capability(console, Capability::Console, " console");
    print_capability(console, Capability::TimerRead, " timer");
    print_capability(console, Capability::TaskInspect, " task_inspect");
    print_capability(console, Capability::TaskControl, " task_control");
    print_capability(console, Capability::DeviceIo, " device_io");
    print_capability(console, Capability::InterruptControl, " irq");
    print_capability(console, Capability::MemoryInspect, " memory");
    print_capability(console, Capability::FileRead, " file_read");
    print_capability(console, Capability::FileWrite, " file_write");
    print_capability(console, Capability::Ipc, " ipc");
    print_capability(console, Capability::ProcessCreate, " process_create");
    print_capability(console, Capability::NetworkIo, " network_io");
    print_capability(console, Capability::StorageIo, " storage_io");
    print_capability(console, Capability::DisplayIo, " display_io");
    print_capability(console, Capability::InputIo, " input_io");
    console.newline();
}

fn cmd_memory(console: &mut Console<'_>) {
    let stats = memory::stats();
    console.print("memory regions: ");
    print_u64(console, stats.usable_regions as u64);
    console.print("  frames: ");
    print_u64(console, stats.total_frames);
    console.print("  used: ");
    print_u64(console, stats.allocated_frames);
    console.print("  free: ");
    print_u64(console, stats.remaining_frames);
    console.print("  numa domains: ");
    print_u64(console, stats.numa_domains as u64);
    console.newline();
}

fn cmd_heap(console: &mut Console<'_>) {
    let stats = heap::stats();
    console.print("heap start: ");
    print_hex_u64(console, stats.start);
    console.print("  size: ");
    print_u64(console, stats.size as u64);
    console.print("  used: ");
    print_u64(console, stats.allocated_bytes as u64);
    console.print("  free: ");
    print_u64(console, stats.free_bytes as u64);
    console.print("  allocs: ");
    print_u64(console, stats.allocations as u64);
    console.newline();
}

fn cmd_paging(console: &mut Console<'_>) {
    let stats = paging::stats();
    console.print("paging: ");
    print_u64(console, stats.successful_translations as u64);
    console.print("/");
    print_u64(console, stats.tested_translations as u64);
    console.print("  l4: ");
    print_hex_u64(console, stats.level_4_frame);
    console.print("  offset: ");
    print_hex_u64(console, stats.physical_memory_offset);
    console.print("  map: ");
    console.println(if stats.mapping_test_passed {
        "ok"
    } else {
        "failed"
    });
}

fn cmd_bench(console: &mut Console<'_>) {
    let delta = benchmark::sample();
    if !delta.baseline_ready {
        console.println("benchmark baseline captured");
        return;
    }
    console.print("bench ticks: ");
    print_u64(console, delta.ticks);
    console.print("  switches: ");
    print_u64(console, delta.context_switches);
    console.print("  preemptions: ");
    print_u64(console, delta.preemptions);
    console.print("  idle: ");
    print_u64(console, delta.idle_heartbeats);
    console.print("  frames: ");
    print_i64(console, delta.frame_change);
    console.print("  heap_bytes: ");
    print_i64(console, delta.heap_byte_change);
    console.print("  allocs: ");
    print_u64(console, delta.heap_allocations);
    console.newline();
}

fn cmd_fs(console: &mut Console<'_>) {
    print_mount_info(console);
    if let Some(stats) = crate::ata::with_primary_master(|disk| disk.stats()) {
        console.print("buffer cache: hits=");
        print_u64(console, stats.hits);
        console.print(" misses=");
        print_u64(console, stats.misses);
        console.print(" writebacks=");
        print_u64(console, stats.writebacks);
        console.print(" evictions=");
        print_u64(console, stats.evictions);
        console.newline();
        console.print("sectors: resident=");
        print_u64(console, stats.resident as u64);
        console.print(" dirty=");
        print_u64(console, stats.dirty as u64);
        console.print(" capacity=");
        print_u64(console, stats.capacity as u64);
        console.newline();
    } else {
        console.println("buffer cache: no ATA device");
    }
    let pages = storage::page_cache_stats();
    console.print("file pages: hits=");
    print_u64(console, pages.hits);
    console.print(" misses=");
    print_u64(console, pages.misses);
    console.print(" evictions=");
    print_u64(console, pages.evictions);
    console.print(" resident=");
    print_u64(console, pages.resident as u64);
    console.print(" capacity=");
    print_u64(console, pages.capacity as u64);
    console.newline();
    console.print("vfs nodes: ");
    print_u64(console, vfs::node_count() as u64);
    console.print("  open-file descriptions: ");
    print_u64(console, vfs::open_file_description_count() as u64);
    console.print("  process fds: ");
    print_u64(console, task::open_file_count() as u64);
    console.newline();
}

fn cmd_mount(arg: &str, console: &mut Console<'_>) {
    let target = arg.trim();
    if target.is_empty() {
        print_mount_info(console);
        return;
    }
    if target != "/mnt" {
        console.println("usage: mount [/mnt]");
        return;
    }
    match storage::remount_mnt() {
        storage::MountStatus::Mounted(count) => {
            console.print("mount: /mnt mounted; imported ");
            print_u64(console, count as u64);
            console.println(" entrie(s)");
        }
        storage::MountStatus::NoDevice => console.println("mount: no ATA disk"),
        storage::MountStatus::NotFat32 => console.println("mount: not a FAT32 volume"),
        storage::MountStatus::Failed => console.println("mount: failed"),
    }
    print_mount_info(console);
}

fn cmd_umount(arg: &str, console: &mut Console<'_>) {
    let target = arg.trim();
    if !target.is_empty() && target != "/mnt" {
        console.println("usage: umount /mnt");
        return;
    }
    match storage::unmount_mnt() {
        Ok(()) => console.println("umount: /mnt unmounted"),
        Err(storage::MountControlError::NoDevice) => console.println("umount: no ATA disk"),
        Err(storage::MountControlError::NotMounted) => {
            console.println("umount: /mnt is not mounted")
        }
        Err(storage::MountControlError::SyncFailed) => {
            console.println("umount: sync failed; /mnt remains mounted")
        }
        Err(storage::MountControlError::Failed) => console.println("umount: failed"),
    }
}

fn cmd_df(arg: &str, console: &mut Console<'_>) {
    let target = arg.trim();
    if !target.is_empty() && target != "/mnt" {
        console.println("usage: df /mnt");
        return;
    }
    match storage::df_mnt() {
        Ok(info) => {
            let used = info.total_clusters.saturating_sub(info.free_clusters);
            let total_bytes = info.total_clusters as u64 * info.bytes_per_cluster as u64;
            let free_bytes = info.free_clusters as u64 * info.bytes_per_cluster as u64;
            console.print("df /mnt: clusters total=");
            print_u64(console, info.total_clusters as u64);
            console.print(" used=");
            print_u64(console, used as u64);
            console.print(" free=");
            print_u64(console, info.free_clusters as u64);
            console.print(" cluster_bytes=");
            print_u64(console, info.bytes_per_cluster as u64);
            console.newline();
            console.print("bytes: total=");
            print_u64(console, total_bytes);
            console.print(" free=");
            print_u64(console, free_bytes);
            console.newline();
            console.print("fsinfo: free_hint=");
            if let Some(hint) = info.fs_info_free_count {
                print_u64(console, hint as u64);
            } else {
                console.print("unknown");
            }
            console.print(" next_hint=");
            if let Some(next) = info.fs_info_next_free {
                print_u64(console, next as u64);
            } else {
                console.print("unknown");
            }
            console.print(" status=");
            console.println(if info.fs_info_matches {
                "ok"
            } else {
                "mismatch"
            });
        }
        Err(err) => print_space_error("df", err, console),
    }
}

fn cmd_fscheck(arg: &str, console: &mut Console<'_>) {
    let target = arg.trim();
    if !target.is_empty() && target != "/mnt" {
        console.println("usage: fscheck /mnt");
        return;
    }
    match storage::fscheck_mnt() {
        Ok(report) => {
            console.print("fscheck /mnt: ok files=");
            print_u64(console, report.files as u64);
            console.print(" directories=");
            print_u64(console, report.directories as u64);
            console.print(" free_clusters=");
            print_u64(console, report.free_clusters as u64);
            console.print("/");
            print_u64(console, report.total_clusters as u64);
            console.print(" fsinfo=");
            console.println(if report.fs_info_matches {
                "ok"
            } else {
                "mismatch"
            });
        }
        Err(err) => print_space_error("fscheck", err, console),
    }
}

fn print_mount_info(console: &mut Console<'_>) {
    let info = storage::mount_info();
    console.print("/mnt: ");
    console.print(match info.status {
        storage::MountLifecycleStatus::Unknown => "unknown",
        storage::MountLifecycleStatus::NoDevice => "no-device",
        storage::MountLifecycleStatus::NotFat32 => "not-fat32",
        storage::MountLifecycleStatus::Mounted => "mounted",
        storage::MountLifecycleStatus::Failed => "failed",
        storage::MountLifecycleStatus::Unmounted => "unmounted",
    });
    console.print(" imported=");
    print_u64(console, info.imported_entries as u64);
    console.print(" dirty=");
    console.print(if info.dirty { "yes" } else { "no" });
    console.print(" syncs=");
    print_u64(console, info.sync_count);
    console.newline();
}

fn print_space_error(command: &str, err: storage::SpaceError, console: &mut Console<'_>) {
    console.print(command);
    console.println(match err {
        storage::SpaceError::Unmounted => ": /mnt is not mounted",
        storage::SpaceError::NoDevice => ": no ATA disk",
        storage::SpaceError::NotFat32 => ": not a FAT32 volume",
        storage::SpaceError::Failed => ": failed",
    });
}
fn cmd_ls(path: &str, console: &mut Console<'_>) {
    let Some(path) = shell_resolve(path) else {
        console.println("ls: bad path");
        return;
    };
    // Pull directory listing for /mnt if needed.
    if touches_mnt(&path) {
        if let Err(err) = storage::ensure_path(&path) {
            print_mount_lookup_error("ls", err, console);
            return;
        }
    }
    match vfs::stat(&path) {
        Ok(stat) if stat.kind == vfs::NodeKind::Directory => {}
        Ok(_) => {
            // Listing a file: show its name only.
            console.print("f ");
            if let Some(name) = path.rsplit('/').next() {
                console.println(if name.is_empty() { path.as_str() } else { name });
            }
            return;
        }
        Err(_) => {
            console.println("ls: not found");
            return;
        }
    }
    let mut index = 0usize;
    let mut any = false;
    while let Ok(entry) = vfs::readdir(&path, index) {
        any = true;
        console.print(match entry.kind {
            vfs::NodeKind::Directory => "d ",
            vfs::NodeKind::File => "f ",
        });
        console.println(entry.name_str());
        index += 1;
        if index > 128 {
            break;
        }
    }
    if !any {
        console.println("(empty)");
    }
}
fn cmd_cat(path: &str, console: &mut Console<'_>) {
    let Some(path) = shell_resolve(path) else {
        console.println("cat: bad path");
        return;
    };
    if touches_mnt(&path) {
        if let Err(err) = storage::ensure_path(&path) {
            print_mount_lookup_error("cat", err, console);
            return;
        }
    }
    let Ok(file) = vfs::open(&path) else {
        console.println("cat: open failed");
        return;
    };
    let mut buffer = [0u8; 512];
    let mut total = 0usize;
    loop {
        match vfs::read(file, &mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                total += length;
                match core::str::from_utf8(&buffer[..length]) {
                    Ok(text) => console.print(text),
                    Err(_) => {
                        console.println("\ncat: binary or invalid utf-8");
                        break;
                    }
                }
            }
            Err(_) => {
                console.println("\ncat: read failed");
                break;
            }
        }
    }
    if total == 0 {
        console.println("(empty)");
    } else {
        // Ensure trailing newline for tidy prompt.
        // (best-effort; we may not know last char)
        console.newline();
    }
    let _ = vfs::close_open_file(file);
}
fn cmd_write(arg: &str, console: &mut Console<'_>) {
    // write <path> <text...>
    let arg = arg.trim();
    if arg.is_empty() {
        console.println("usage: write <path> <text>");
        return;
    }
    let (path_part, text) = split_command(arg);
    if path_part.is_empty() {
        console.println("usage: write <path> <text>");
        return;
    }
    let Some(path) = shell_resolve(path_part) else {
        console.println("write: bad path");
        return;
    };
    if touches_mnt(&path) && !storage::mnt_mounted() {
        console.println("write: /mnt is not mounted");
        return;
    }
    if is_mnt_child(&path) {
        if let Some(parent) = shell_parent_path(&path) {
            if touches_mnt(&parent) {
                if let Err(err) = storage::ensure_path(&parent) {
                    print_mount_lookup_error("write", err, console);
                    return;
                }
            }
        }
    }
    match vfs::write_file(&path, text.as_bytes()) {
        Ok(()) => {
            if touches_mnt(&path) {
                storage::mark_mnt_dirty();
            }
            console.print("wrote ");
            print_u64(console, text.len() as u64);
            console.println(" bytes");
        }
        Err(vfs::Error::ReadOnly) => console.println("write: read-only"),
        Err(vfs::Error::Full) => console.println("write: full or too large"),
        Err(vfs::Error::NotFound) => console.println("write: parent missing"),
        Err(vfs::Error::AlreadyExists) => console.println("write: path is a directory"),
        Err(_) => console.println("write: failed"),
    }
}
fn cmd_rename(args: &str, console: &mut Console<'_>) {
    let mut args = args.split_whitespace();
    let (Some(old), Some(new), None) = (args.next(), args.next(), args.next()) else {
        console.println("usage: rename <old> <new>");
        return;
    };
    let (Some(old), Some(new)) = (shell_resolve(old), shell_resolve(new)) else {
        console.println("rename: bad path");
        return;
    };

    if touches_mnt(&old) || touches_mnt(&new) {
        if !storage::mnt_mounted() {
            console.println("rename: /mnt is not mounted");
            return;
        }
        if !is_mnt_child(&old) || !is_mnt_child(&new) {
            console.println("rename: cross-mount not supported");
            return;
        }
        if let Err(err) = storage::ensure_path(&old) {
            print_mount_lookup_error("rename", err, console);
            return;
        }
        if let Some(parent) = shell_parent_path(&new) {
            if touches_mnt(&parent) {
                if let Err(err) = storage::ensure_path(&parent) {
                    print_mount_lookup_error("rename", err, console);
                    return;
                }
            }
        }
        match vfs::can_rename(&old, &new) {
            Ok(()) => {}
            Err(err) => {
                print_rename_vfs_error(err, console);
                return;
            }
        }
        match storage::rename_path(&old, &new) {
            Ok(()) => {}
            Err(err) => {
                print_rename_storage_error(err, console);
                return;
            }
        }
    }

    match vfs::rename(&old, &new) {
        Ok(()) => console.println("renamed"),
        Err(err) => print_rename_vfs_error(err, console),
    }
}

fn cmd_rm(path: &str, console: &mut Console<'_>) {
    let Some(path) = shell_resolve(path) else {
        console.println("rm: bad path");
        return;
    };
    if path == "/mnt" {
        console.println("rm: refused");
        return;
    }

    if is_mnt_child(&path) {
        if !storage::mnt_mounted() {
            console.println("rm: /mnt is not mounted");
            return;
        }
        if let Err(err) = storage::ensure_path(&path) {
            print_mount_lookup_error("rm", err, console);
            return;
        }
        match vfs::can_remove(&path) {
            Ok(()) => {}
            Err(err) => {
                print_rm_vfs_error(err, console);
                return;
            }
        }
        if let Err(err) = vfs::prepare_remove(&path) {
            print_rm_vfs_error(err, console);
            return;
        }
        match storage::delete_path(&path) {
            Ok(()) => {}
            Err(err) => {
                print_rm_storage_error(err, console);
                return;
            }
        }
    }

    match vfs::remove(&path) {
        Ok(()) => console.println("removed"),
        Err(err) => print_rm_vfs_error(err, console),
    }
}
fn cmd_mkdir(path: &str, console: &mut Console<'_>) {
    let Some(path) = shell_resolve(path) else {
        console.println("mkdir: bad path");
        return;
    };
    if touches_mnt(&path) && !storage::mnt_mounted() {
        console.println("mkdir: /mnt is not mounted");
        return;
    }
    if is_mnt_child(&path) {
        if let Some(parent) = shell_parent_path(&path) {
            if touches_mnt(&parent) {
                if let Err(err) = storage::ensure_path(&parent) {
                    print_mount_lookup_error("mkdir", err, console);
                    return;
                }
            }
        }
    }
    match vfs::mkdir(&path) {
        Ok(()) => {
            if is_mnt_child(&path) {
                match storage::persist_directory(&path) {
                    Ok(()) => console.println("ok"),
                    Err(storage::PersistError::Unmounted) => {
                        let _ = vfs::remove(&path);
                        console.println("mkdir: /mnt is not mounted")
                    }
                    Err(storage::PersistError::NoDevice) => {
                        let _ = vfs::remove(&path);
                        console.println("mkdir: no ATA disk")
                    }
                    Err(storage::PersistError::BadName) => {
                        let _ = vfs::remove(&path);
                        console.println("mkdir: FAT 8.3 path required")
                    }
                    Err(_) => {
                        let _ = vfs::remove(&path);
                        console.println("mkdir: disk persist failed")
                    }
                }
            } else {
                console.println("ok");
            }
        }
        Err(vfs::Error::AlreadyExists) => console.println("mkdir: exists"),
        Err(vfs::Error::NotFound) => console.println("mkdir: parent missing"),
        Err(vfs::Error::Full) => console.println("mkdir: vfs full"),
        Err(_) => console.println("mkdir: failed"),
    }
}
fn cmd_stat(path: &str, console: &mut Console<'_>) {
    let Some(path) = shell_resolve(path) else {
        console.println("stat: bad path");
        return;
    };
    if touches_mnt(&path) {
        if let Err(err) = storage::ensure_path(&path) {
            print_mount_lookup_error("stat", err, console);
            return;
        }
    }
    match vfs::stat(&path) {
        Ok(stat) => {
            console.print("path: ");
            console.println(&path);
            console.print("kind: ");
            console.println(match stat.kind {
                vfs::NodeKind::File => "file",
                vfs::NodeKind::Directory => "directory",
            });
            console.print("size: ");
            print_u64(console, stat.size as u64);
            console.newline();
            console.print("writable: ");
            console.println(if stat.writable { "yes" } else { "no" });
        }
        Err(_) => console.println("stat: not found"),
    }
}
fn cmd_userland_command(verb: &str, arg: &str, console: &mut Console<'_>) -> bool {
    if verb.is_empty() || verb.len() > 64 {
        return false;
    }
    let _ = install_userland();
    let path = if verb.as_bytes().contains(&b'/') {
        alloc::string::String::from(verb)
    } else {
        alloc::format!("/bin/{verb}")
    };
    if vfs::stat(&path).is_err() {
        return false;
    }
    let mut image = alloc::vec![0u8; vfs::NODE_CAPACITY];
    let Ok(len) = vfs::read_all(&path, &mut image) else {
        console.println("command: read failed");
        return true;
    };
    let program = if arg.is_empty() {
        userspace::load_elf_with_argv(&image[..len], &[path.as_str()])
    } else {
        userspace::load_elf_with_argv(&image[..len], &[path.as_str(), arg])
    };
    let Some(program) = program else {
        console.println("command: ELF load failed");
        return true;
    };
    let (cursor_x, cursor_y) = console.cursor_position();
    terminal::set_cursor_position(cursor_x, cursor_y);

    match task::spawn_user_process("kcmd", program) {
        Ok((pid, _)) => {
            terminal::set_foreground(pid.as_u64());
            while !task::process_exited(pid) {
                task::yield_now();
            }
            let (cursor_x, cursor_y) = terminal::cursor_position();
            console.set_cursor_position(cursor_x, cursor_y);
        }
        Err(_) => console.println("command: spawn failed"),
    }
    true
}

fn cmd_run(path: &str, console: &mut Console<'_>, foreground: bool) {
    let Some(path) = shell_resolve(path) else {
        console.println("run: bad path");
        return;
    };
    if touches_mnt(&path) {
        if let Err(err) = storage::ensure_path(&path) {
            print_mount_lookup_error("run", err, console);
            return;
        }
    }
    let mut image = alloc::vec![0u8; vfs::NODE_CAPACITY];
    let Ok(len) = vfs::read_all(&path, &mut image) else {
        console.println("run: read failed");
        return;
    };
    let Some(program) = userspace::load_elf_with_argv(&image[..len], &[path.as_str()]) else {
        console.println("run: elf load failed");
        return;
    };
    if foreground {
        let (x, y) = console.cursor_position();
        terminal::set_cursor_position(x, y);
    }
    match task::spawn_user_process("run", program) {
        Ok((id, context)) => {
            if foreground {
                terminal::set_foreground(id.as_u64());
                while !task::process_exited(id) {
                    task::yield_now();
                }
                let (x, y) = terminal::cursor_position();
                console.set_cursor_position(x, y);
                let _ = task::wait_process(id.as_u64());
                return;
            }
            console.print("running pid=");
            print_u64(console, id.as_u64());
            console.print(" entry=");
            print_hex_u64(console, context.entry);
            console.newline();
        }
        Err(_) => console.println("run: spawn failed"),
    }
}
fn cmd_sh(console: &mut Console<'_>) {
    let installed = install_userland();
    if installed != 24 {
        console.print("sh: userland incomplete (");
        print_u64(console, installed);
        console.println("/24)");
        return;
    }

    crate::serial::write_line(format_args!("[SH] building userspace shell image"));
    let Some(program) = userspace::create_shell_process() else {
        console.println("sh: image failed");
        crate::serial::write_line(format_args!("[SH] create_shell_process failed"));
        return;
    };

    crate::serial::write_line(format_args!("[SH] image ready; spawning process"));
    match task::spawn_user_process("sh", program) {
        Ok((id, context)) => {
            serial_user_shell_start(id.as_u64(), context.entry);
            terminal::set_foreground(id.as_u64());
            console.println("starting userspace shell...");
            crate::serial::write_line(format_args!(
                "[SH] foreground assigned to pid={}; dispatching immediately",
                id.as_u64()
            ));

            // Interactive userspace must NOT be waited on here. cmd_sh() is
            // called from the kernel's main event loop; blocking here prevents
            // the loop from continuing network polling, syscall servicing and
            // scheduler preemption. Foreground ownership already prevents the
            // diagnostic shell from consuming keyboard input, so return to the
            // main loop immediately and let /bin/sh run asynchronously.
            let (cursor_x, cursor_y) = console.cursor_position();
            terminal::set_cursor_position(cursor_x, cursor_y);
            task::yield_now();

            crate::serial::write_line(format_args!(
                "[SH] userspace shell scheduled asynchronously; foreground_active={}",
                terminal::foreground_active()
            ));
        }
        Err(_) => {
            console.println("sh: spawn failed");
            crate::serial::write_line(format_args!("[SH] spawn_user_process failed"));
        }
    }
}

fn serial_user_shell_start(pid: u64, entry: u64) {
    crate::serial::write_line(format_args!(
        "[TTY] userspace shell foreground pid={} entry={:#x}",
        pid, entry
    ));
}

fn cmd_init(console: &mut Console<'_>) {
    let Some(program) = userspace::create_init_process() else {
        console.println("init: image failed");
        return;
    };
    match task::spawn_user_process("init", program) {
        Ok((id, context)) => {
            console.print("init pid=");
            print_u64(console, id.as_u64());
            console.print(" entry=");
            print_hex_u64(console, context.entry);
            console.print(" stack=");
            print_hex_u64(console, context.stack_top);
            console.newline();
        }
        Err(_) => console.println("init: spawn failed"),
    }
}

fn cmd_user(console: &mut Console<'_>) {
    let Some(program) = userspace::create_stub_process() else {
        console.println("user: image mapping failed");
        return;
    };
    match task::spawn_user_process("usermode", program) {
        Ok((id, context)) => {
            console.print("user pid=");
            print_u64(console, id.as_u64());
            console.print(" entry=");
            print_hex_u64(console, context.entry);
            console.print(" stack=");
            print_hex_u64(console, context.stack_top);
            console.newline();
        }
        Err(_) => console.println("user: spawn failed"),
    }
}

fn is_mnt_child(path: &str) -> bool {
    path.starts_with("/mnt/")
}

fn touches_mnt(path: &str) -> bool {
    path == "/mnt" || path.starts_with("/mnt/")
}

fn shell_parent_path(path: &str) -> Option<alloc::string::String> {
    if !path.starts_with('/') || path == "/" {
        return None;
    }
    let index = path.rfind('/')?;
    if index == 0 {
        Some(alloc::string::String::from("/"))
    } else {
        Some(alloc::string::String::from(&path[..index]))
    }
}

fn print_mount_lookup_error(command: &str, err: storage::EnsureError, console: &mut Console<'_>) {
    console.print(command);
    console.println(match err {
        storage::EnsureError::Unmounted => ": /mnt is not mounted",
        storage::EnsureError::NotFound => ": not found on volume",
        storage::EnsureError::NoDevice => ": no block device",
        storage::EnsureError::NotFat32 => ": not a FAT32 volume",
        storage::EnsureError::TooLarge => ": file too large",
        storage::EnsureError::InvalidPath => ": bad path",
        _ => ": mount lookup failed",
    });
}
fn print_rename_vfs_error(err: vfs::Error, console: &mut Console<'_>) {
    match err {
        vfs::Error::NotFound => console.println("rename: source or destination parent missing"),
        vfs::Error::AlreadyExists => console.println("rename: destination exists"),
        vfs::Error::InvalidPath => console.println("rename: invalid path or destination"),
        vfs::Error::ReadOnly => console.println("rename: refused"),
        _ => console.println("rename: failed"),
    }
}

fn print_rename_storage_error(err: storage::MutationError, console: &mut Console<'_>) {
    match err {
        storage::MutationError::Unmounted => console.println("rename: /mnt is not mounted"),
        storage::MutationError::NotSupported => {
            console.println("rename: cross-mount not supported")
        }
        storage::MutationError::NotFound => {
            console.println("rename: source or destination parent missing")
        }
        storage::MutationError::NoDevice => console.println("rename: no ATA disk"),
        storage::MutationError::BadName => console.println("rename: FAT 8.3 path required"),
        storage::MutationError::AlreadyExists => console.println("rename: destination exists"),
        storage::MutationError::NotEmpty => console.println("rename: directory not empty"),
        storage::MutationError::ReadOnly => console.println("rename: refused"),
        storage::MutationError::Failed => console.println("rename: disk update failed"),
    }
}

fn print_rm_vfs_error(err: vfs::Error, console: &mut Console<'_>) {
    match err {
        vfs::Error::NotFound => console.println("rm: not found"),
        vfs::Error::ReadOnly => console.println("rm: refused"),
        vfs::Error::NotEmpty => console.println("rm: directory not empty"),
        _ => console.println("rm: failed"),
    }
}

fn print_rm_storage_error(err: storage::MutationError, console: &mut Console<'_>) {
    match err {
        storage::MutationError::Unmounted => console.println("rm: /mnt is not mounted"),
        storage::MutationError::NotSupported => console.println("rm: refused"),
        storage::MutationError::NotFound => console.println("rm: not found"),
        storage::MutationError::NoDevice => console.println("rm: no ATA disk"),
        storage::MutationError::BadName => console.println("rm: FAT 8.3 path required"),
        storage::MutationError::AlreadyExists => console.println("rm: failed"),
        storage::MutationError::NotEmpty => console.println("rm: directory not empty"),
        storage::MutationError::ReadOnly => console.println("rm: refused"),
        storage::MutationError::Failed => console.println("rm: disk update failed"),
    }
}

fn shell_resolve(path: &str) -> Option<alloc::string::String> {
    let absolute = if path.starts_with('/') {
        alloc::string::String::from(path)
    } else {
        let cwd = cwd_owned();
        let mut joined = alloc::string::String::new();
        if cwd == "/" {
            joined.push('/');
            joined.push_str(path);
        } else {
            joined.push_str(&cwd);
            joined.push('/');
            joined.push_str(path);
        }
        joined
    };
    let mut stack = alloc::vec::Vec::new();
    for component in absolute.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            let _ = stack.pop();
            continue;
        }
        stack.push(component);
    }
    if stack.is_empty() {
        return Some(alloc::string::String::from("/"));
    }
    let mut out = alloc::string::String::from("/");
    for (i, part) in stack.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(part);
    }
    Some(out)
}

fn shell_chdir(path: &str) -> bool {
    let Some(absolute) = shell_resolve(path) else {
        return false;
    };
    if absolute == "/mnt" || absolute.starts_with("/mnt/") {
        let _ = storage::ensure_path(&absolute);
    }
    match vfs::stat(&absolute) {
        Ok(stat) if stat.kind == vfs::NodeKind::Directory => state().set_cwd(&absolute),
        _ => false,
    }
}

fn split_command(command: &str) -> (&str, &str) {
    let command = command.trim();
    if command.is_empty() {
        return ("", "");
    }
    match command.find(char::is_whitespace) {
        Some(idx) => {
            let verb = &command[..idx];
            let arg = command[idx..].trim_start();
            (verb, arg)
        }
        None => (command, ""),
    }
}

fn lowercase<'a>(verb: &'a str, buf: &'a mut [u8; 32]) -> &'a str {
    if verb.len() > buf.len() {
        return verb;
    }
    for (i, b) in verb.bytes().enumerate() {
        buf[i] = if b.is_ascii_uppercase() { b + 32 } else { b };
    }
    core::str::from_utf8(&buf[..verb.len()]).unwrap_or(verb)
}

fn demo_task() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

fn authorize(capability: Capability, console: &mut Console<'_>) -> bool {
    if task::current_has(capability) {
        return true;
    }
    console.println("permission denied");
    false
}

fn print_capability(console: &mut Console<'_>, capability: Capability, name: &str) {
    if task::current_has(capability) {
        console.print(name);
    }
}

fn print_u64(console: &mut Console<'_>, mut value: u64) {
    if value == 0 {
        console.print("0");
        return;
    }
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    while value > 0 {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        console.put_char(digits[len] as char);
    }
}

fn print_i64(console: &mut Console<'_>, value: i64) {
    if value < 0 {
        console.print("-");
        print_u64(console, value.unsigned_abs());
    } else {
        print_u64(console, value as u64);
    }
}

fn print_hex_u64(console: &mut Console<'_>, value: u64) {
    console.print("0x");
    let mut started = false;
    for shift in (0..64).step_by(4).rev() {
        let nibble = ((value >> shift) & 0xf) as u8;
        if nibble != 0 || started || shift == 0 {
            started = true;
            let ch = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + (nibble - 10)
            };
            console.put_char(ch as char);
        }
    }
}
