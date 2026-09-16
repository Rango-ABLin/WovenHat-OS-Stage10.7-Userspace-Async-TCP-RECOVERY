WovenHat OS — Stage 10.7 → Version 1.0 Master Roadmap
Phase I — Finish the asynchronous kernel foundation
Stage 10.7 — Asynchronous TCP

Current stage

Complete:

asynchronous TCP connect
asynchronous TCP send
asynchronous TCP receive
EOF handling
connection-state transitions
socket generation/pinning
close while operations are in flight
cancellation
process teardown
SMP-safe network worker
1/2/4 CPU testing
real QEMU host↔guest TCP tests

Exit condition: Stage 10.7 acceptance passes completely.

Stage 10.8 — Unified Completion Port / Wait-Many

This should be the next major architectural feature.

Implement a per-process completion mechanism similar conceptually to:

epoll
kqueue
IOCP
io_uring completion queues

A process must be able to wait for many asynchronous operations simultaneously.

Example:

File read ─────┐
TCP receive ───┤
Timer ─────────┼──> Completion Port ──> Application
IPC message ───┤
Block I/O ─────┘

Implement:

CompletionPort object
completion queue
associate async operation with port
wait-one
wait-many
batch completion dequeue
cancellation events
timeout support
generation-safe handles
SMP-safe producer/consumer behavior
bounded queues
overflow behavior
owner teardown

This becomes the basis for GUI event loops, servers and AI services.

Stage 10.9 — Timers + Asynchronous Events

Implement proper application-visible:

one-shot timers
periodic timers
monotonic clock
deadlines
timeout cancellation
sleep-until
timer completion events
event objects
wait-many integration

No application should need busy waiting.

Stage 10.10 — Mature Async Runtime

Unify:

BLOCK
FILE
TCP
UDP
IPC
TIMERS
DEVICES
EVENTS

under the Stage 10.8 completion architecture.

At this point Stage 10 is closed.

Phase II — Process and userspace architecture
Stage 11 — Production Userspace
11.1 — Process Model

Develop a real process abstraction:

Process
 ├── Address Space
 ├── Threads
 ├── Capability Table
 ├── File Handles
 ├── IPC Handles
 ├── Completion Ports
 ├── Security Domain
 └── Resource Limits

Add:

PID management
parent/child relationships
exit status
waitpid-like semantics
process groups
clean teardown
11.2 — Multithreaded Processes

Implement:

thread creation
thread termination
join
TLS
per-thread stack
per-thread registers
thread-local errno/state
11.3 — Signals / Structured Notifications

Rather than blindly cloning Unix signals, design safer WovenHat notifications.

Support:

terminate
suspend
resume
child exit
exceptions
user-defined notifications

Prefer structured event delivery.

11.4 — Userspace Runtime

Create:

libwoven

The fundamental WovenHat userspace library.

Eventually expose:

woven::process
woven::thread
woven::fs
woven::net
woven::ipc
woven::async_io
woven::security
woven::time
woven::graphics
11.5 — Program Loader

Strengthen ELF loading:

PIE
ASLR
dynamic relocation
shared libraries
TLS
stack protection
W^X
RELRO-style protection
Phase III — Filesystem and storage
Stage 12 — Production Storage Stack
12.1 — VFS 2.0

Build a clean filesystem abstraction:

Application
     ↓
VFS
 ┌───┼────┬───────┐
WFS FAT tmpfs procfs
12.2 — WovenFS

Create WovenHat's native filesystem.

Target features:

64-bit filesystem
directories
metadata
timestamps
permissions
extended attributes
checksums
journaling or copy-on-write
crash consistency
12.3 — Filesystem Encryption

Add:

encrypted volumes
per-user keys
secure key handling
authenticated encryption

The kernel now provides the authenticated-encryption envelope and a bounded,
generation-safe revocable key vault. Measured key provisioning, persistent key
storage, rotation policy, and encrypted-volume mount integration remain open.
12.4 — Snapshots

Implement filesystem snapshots.

This enables one of WovenHat's future signature features:

Undo System Change

If an update or application damages the system, WovenHat can roll back.

12.5 — Storage Management

Implement:

partitions
GPT
mount manager
removable media
filesystem detection
disk information
Phase IV — Driver architecture
Stage 13 — WovenDriver Framework

Do not attempt to write a separate driver for every device on Earth.

Instead build a reusable driver architecture.

13.1 — Driver Framework

Create:

Driver Manager
      ↓
Device Bus
 ├── PCIe
 ├── USB
 ├── ACPI
 └── Virtual
      ↓
Drivers

Support:

device discovery
matching
binding
initialization
hotplug
suspend/resume
driver isolation
13.2 — PCIe

Production PCI/PCIe support.

13.3 — NVMe

Native NVMe driver.

13.4 — AHCI/SATA

For SATA SSDs/HDDs.

13.5 — USB Core

Implement:

xHCI
USB enumeration
endpoints
transfers
hotplug
13.6 — USB HID

Support:

keyboards
mice
touchpads
13.7 — Input Framework

Unified:

Keyboard
Mouse
Touchpad
Touchscreen
Pen
Game controller
       ↓
WovenInput
13.8 — Audio

Create WovenAudio.

Initially support:

Intel HDA
playback
recording
volume
streams
13.9 — Wi-Fi Framework

Start with selected hardware families rather than every Wi-Fi adapter.

Implement:

scanning
association
WPA2/WPA3 integration
network selection
reconnect
13.10 — Bluetooth

Later add:

Bluetooth controller
HID
audio
pairing
Phase V — Networking
Stage 14 — WovenNet

Take the existing networking work to production level.

14.1

Stable:

UDP
TCP
DNS
DHCP
ICMP
14.2

Add:

IPv6
DHCPv6
IPv6 neighbor discovery
14.3

Socket API.

14.4

Routing tables.

14.5

WovenGuard firewall.

14.6

TLS userspace library.

14.7

HTTP/HTTPS client.

14.8

Network Manager service.

Eventually the desktop gets:

Wi-Fi
Ethernet
VPN
DNS
Firewall
Hotspot

from one userspace service.

Phase VI — WovenGuard Security Architecture
Stage 15 — WovenGuard 2.0

You already have an unusually strong security foundation. This phase turns it into a complete OS security architecture.

15.1 — Capability Enforcement Everywhere

Everything sensitive becomes capability-controlled:

Camera
Microphone
Network
Filesystem
GPU
USB
Clipboard
Location
Processes
IPC
AI
15.2 — Application Sandboxes

Each application gets its own security domain.

Example:

Browser
 ├── Network ✓
 ├── Downloads ✓
 ├── Camera ASK
 ├── Microphone ASK
 ├── System files ✗
 └── Other apps' memory ✗
15.3 — Permission Broker

Graphical permission prompts.

15.4 — Signed Applications

Verify software signatures before execution.

15.5 — Secure Boot

Eventually support:

UEFI Secure Boot
      ↓
WovenHat Bootloader
      ↓
Verified Kernel
      ↓
Verified System
15.6 — Measured Boot

TPM-backed measurements.

15.7 — Secrets Vault

Build:

WovenVault

for:

passwords
tokens
encryption keys
Wi-Fi credentials
application secrets
15.8 — Security Ledger

Expand your existing audit ledger into a user-visible security history.

For example:

10:42 Browser requested camera
10:42 Permission granted
10:47 Camera capability revoked
11:03 Unknown USB device blocked
15.9 — Exploit Mitigations

Implement or verify:

ASLR
NX
SMEP
SMAP
stack guards
guard pages
W^X
heap hardening
kernel address randomization
Phase VII — Graphics
Stage 16 — WovenGraphics

This is where WovenHat begins becoming a graphical OS.

16.1 — Graphics Abstraction

Stop treating UEFI framebuffer as the final graphics architecture.

Create:

WovenGraphics
     ↓
Display Driver
     ↓
GPU / framebuffer
16.2 — 2D Renderer

Support:

rectangles
paths
gradients
images
clipping
transforms
alpha blending
16.3 — Font Engine

Implement:

TrueType/OpenType
Unicode
font fallback
anti-aliasing
text shaping

Eventually integrate complex scripts correctly.

16.4 — GPU Acceleration

Initially use software rendering.

Then introduce:

virtio-gpu
hardware-specific GPU work
accelerated surfaces

Do not make NVIDIA/AMD/Intel native GPU support a prerequisite for the first GUI release.

Phase VIII — Window System
Stage 17 — WovenCompositor

This is one of the most important stages.

Architecture:

Applications
     ↓
WovenSurface Protocol
     ↓
WovenCompositor
     ↓
WovenGraphics
     ↓
GPU

Implement:

windows
surfaces
compositing
damage tracking
z-order
transparency
shadows
animations
input routing
focus
multiple monitors
DPI scaling

Security rule:

Applications must never directly control another application's surface.

Phase IX — WovenHat Desktop
Stage 18 — WovenShell

Now build the desktop environment.

8
18.1 — Desktop

Build:

wallpaper
desktop surface
taskbar/dock
system tray
clock
notifications
18.2 — WovenStart

Application launcher/search.

18.3 — Window Experience

Develop WovenHat's own visual identity rather than simply copying Windows.

Consider:

dynamic depth
subtle translucency
adaptive surfaces
smooth physics-based animation
workspace transitions
intelligent tiling
18.4 — Notification Center
18.5 — Quick Settings

Wi-Fi, Bluetooth, sound, brightness, power, VPN and security.

18.6 — Lock Screen
18.7 — Login Manager
18.8 — Multiple Workspaces
18.9 — Multi-monitor Desktop
Phase X — Core Applications
Stage 19 — WovenHat System Applications

Develop native applications:

WovenFiles
WovenTerminal
WovenSettings
WovenMonitor
WovenSecurity
WovenEditor
WovenCalculator
WovenImages
WovenUpdate
WovenStore

The Settings application should expose:

appearance
accounts
networking
devices
applications
privacy
security
updates
storage
accessibility
Phase XI — Application Platform
Stage 20 — WovenSDK

Applications need a stable development platform.

Build:

SDK
application runtime
GUI toolkit
widgets
layout system
accessibility API
notification API
clipboard API
drag-and-drop
file chooser
permission API

Create something conceptually like:

Application::new("My App")
    .window(...)
    .run();
Stage 20.5 — WovenUI

Your official native GUI toolkit.

Components:

Button
Text
TextField
List
Table
Tree
Menu
Dialog
Tabs
Navigation
Image
Video
Canvas
WebView
Phase XII — Software Distribution
Stage 21 — WovenPackages

Build a package format.

For example:

example.woven

Include:

manifest
application
resources
capabilities
signature
version
dependencies

Then build:

WovenStore

with:

installation
updates
removal
signature validation
rollback
sandbox declarations
Phase XIII — System Services
Stage 22 — Service Architecture

Move functionality out of the kernel wherever practical.

Create services such as:

WovenNetwork
WovenAudio
WovenDisplay
WovenInput
WovenPackage
WovenUpdate
WovenSecurity
WovenIdentity
WovenAI

IPC becomes the backbone.

This keeps the kernel smaller and makes components independently replaceable.

Phase XIV — AI-Native WovenHat
Stage 23 — WovenAI

AI should not merely be a chatbot installed on the OS.

Make AI an OS service.

Architecture:

Applications ──┐
Desktop ───────┤
Search ────────┤
Accessibility ─┼── WovenAI Broker
Terminal ──────┤        ↓
Files ─────────┤   AI Providers
Security ──────┘
23.1 — AI Broker

Applications request AI capabilities through controlled IPC.

23.2 — Local Models

Support local inference where hardware permits.

23.3 — Cloud Models

Provider plugins, but credentials stay in WovenVault.

23.4 — AI Capability Security

An AI model gets no automatic access to:

files
camera
microphone
passwords
applications
network

It receives explicit WovenGuard capabilities.

23.5 — Semantic Search

Search:

Applications
Files
Settings
Messages
Documents
Commands
23.6 — Woven Assistant

System assistant capable of executing permitted OS actions.

23.7 — AI Security Monitor

AI can explain suspicious activity, but WovenGuard remains the actual enforcement mechanism.

That distinction is important: AI advises; deterministic kernel/security policy enforces.

Phase XV — Accounts and Identity
Stage 24 — WovenIdentity

Implement:

local users
passwords
groups
sessions
lock/unlock
privilege elevation
user profiles
credential storage

Later:

biometrics
security keys
FIDO2/WebAuthn
enterprise identity
Phase XVI — Power and Hardware Management
Stage 25 — Power Management

Implement:

ACPI poweroff
reboot
sleep
wake
CPU idle
CPU frequency management
battery
thermal monitoring
laptop lid
suspend/resume
Phase XVII — Accessibility and Internationalization
Stage 26 — Accessibility

Accessibility must be architectural rather than added at the end.

Implement:

screen reader APIs
keyboard navigation
high contrast
scaling
magnifier
reduced motion
captions
accessibility tree
Stage 26.5 — Internationalization

Implement:

Unicode
locale
date/time formats
number formats
translations
RTL
keyboard layouts
IMEs

This also creates a path for excellent African-language support, including Chichewa.

Phase XVIII — Updates and Recovery
Stage 27 — WovenUpdate

Updates should be:

signed
atomic
rollback-safe
resumable

Potential architecture:

Current System A
       ↓
install update
       ↓
System B
       ↓
reboot
       ↓
verify
   ↙       ↘
PASS       FAIL
 ↓          ↓
keep B    rollback A
Stage 27.5 — Recovery Environment

Build:

WovenRecovery

capable of:

repairing boot
filesystem checking
restoring snapshots
resetting system
reinstalling
retrieving logs
Phase XIX — Installation
Stage 28 — WovenInstaller

Turn the development image into something people can install.

Implement:

bootable ISO/USB image
graphical installer
disk detection
partitioning
filesystem formatting
bootloader installation
user creation
timezone
keyboard
networking
installation progress
Phase XX — Hardware Expansion
Stage 29 — Real Hardware Qualification

Create a hardware compatibility laboratory.

Test categories:

Intel PCs
AMD PCs
Laptops
Desktops
NVMe
SATA
USB
Ethernet
Wi-Fi
Audio
Intel graphics
AMD graphics
NVIDIA graphics
Touchpads
Webcams
Bluetooth
Multi-monitor

Do not require every device to work before 1.0.

Instead establish:

Tier 1 — officially supported

Tier 2 — expected to work

Tier 3 — experimental

Phase XXI — Performance
Stage 30 — Performance Engineering

Build proper tooling:

kernel profiler
scheduler latency measurements
syscall benchmarks
IPC benchmarks
filesystem benchmarks
network throughput
compositor frame timing
memory-pressure testing
boot profiling

Targets should eventually include:

60/120 Hz desktop
low IPC latency
fast boot
low idle CPU
controlled memory usage
responsive application startup
Phase XXII — Reliability
Stage 31 — WovenHat Torture Suite

This is critical before calling the OS finished.

Automate:

1 CPU
2 CPUs
4 CPUs
8 CPUs
16 CPUs

Stress:

scheduler
memory
IPC
capabilities
filesystems
networking
async I/O
drivers
GUI
process lifecycle

Include randomized failure injection.

Test:

low memory
disk full
network loss
corrupted packets
driver crash
process crash
service restart
sudden shutdown

Run thousands of automated boots.

Phase XXIII — Developer Experience
Stage 32 — WovenDev

Create:

debugger
kernel debugger
crash dumps
structured logging
tracing
performance profiler
SDK documentation
API documentation
examples
application templates

Eventually developers should be able to write WovenHat software without understanding the kernel.

Phase XXIV — Release Engineering
Stage 33 — Alpha

The OS should now:

boot real hardware
install itself
show the desktop
run applications
connect to networks
play audio
manage files
update itself

Expect bugs.

Stage 34 — Beta

Focus on:

compatibility
stability
hardware
accessibility
security audit
performance
application polish

Feature freeze begins.

Stage 35 — Release Candidate

No major architecture changes.

Only:

critical bug fixes
security fixes
compatibility fixes
performance regressions
documentation

Run full acceptance repeatedly.

Stage 36 — WovenHat OS 1.0

This is the point I would call the first version of WovenHat finished enough to be a real general-purpose operating system.

The 1.0 stack would look approximately like this:

┌─────────────────────────────────────────────┐
│               WovenHat Desktop              │
│ WovenFiles │ Settings │ Store │ Assistant   │
├─────────────────────────────────────────────┤
│        WovenUI + WovenSDK + WovenAI         │
├─────────────────────────────────────────────┤
│ WovenCompositor │ Audio │ Network │ Update  │
├─────────────────────────────────────────────┤
│           Userspace System Services         │
├─────────────────────────────────────────────┤
│       WovenGuard Security Architecture      │
├─────────────────────────────────────────────┤
│ IPC │ Async I/O │ VFS │ Processes │ Sockets│
├─────────────────────────────────────────────┤
│ Scheduler │ VM │ SMP │ Timers │ Interrupts │
├─────────────────────────────────────────────┤
│ PCIe │ NVMe │ USB │ HID │ GPU │ Wi-Fi      │
├─────────────────────────────────────────────┤
│             WovenHat Rust Kernel            │
├─────────────────────────────────────────────┤
│                  Hardware                   │
└─────────────────────────────────────────────┘
Instructions to put above this roadmap when feeding it to Codex

Give Codex this as the permanent operating rule:

WOVENHAT OS MASTER DEVELOPMENT DIRECTIVE

Continue development of WovenHat OS according to the supplied Stage 10.7–36
roadmap.

Do NOT skip stages.

For every stage:

1. Inspect the complete existing repository before modifying it.
2. Preserve all previously accepted architecture and behavior.
3. Implement the stage as production architecture, not a test-only hack.
4. Prefer safe Rust. Keep unsafe Rust small, documented and auditable.
5. Use assembly only where direct CPU architecture requires it.
6. Do not weaken WovenGuard or grant broad capabilities merely to make tests pass.
7. Do not use busy loops where event-driven blocking is appropriate.
8. Avoid global locks where they damage SMP scalability.
9. Protect all kernel objects against stale handles, generation reuse,
   process teardown and concurrent access.
10. Every userspace pointer must be validated and must never be retained
    asynchronously without copying/pinning through a safe kernel-owned mechanism.
11. Test failure paths as well as successful paths.
12. Preserve 1/2/4 CPU acceptance and expand to 8+ CPUs as the SMP architecture matures.
13. Every new stage must run all relevant previous-stage regression tests.
14. Never hide failures by merely increasing timeouts, disabling tests,
    suppressing warnings or weakening assertions.
15. cargo build must pass.
16. cargo clippy -- -D warnings must pass.
17. Host/unit tests must pass.
18. QEMU acceptance tests must pass.
19. Automatically preserve serial/QEMU/failure logs.
20. Do not ask the developer to repeatedly diagnose problems that can be
    determined from source and generated logs.
21. When a failure occurs, inspect the evidence, identify the root cause,
    fix it and rerun automated validation.
22. Update the WovenHat OS Architecture & Codebase Guide for every stage.
23. Document every new file, important structure, syscall, capability,
    subsystem relationship and control/data flow.
24. Update the roadmap/status document after every accepted stage.
25. Commit each accepted stage separately to Git with a descriptive commit.
26. Do not proceed to the next stage until the current stage's acceptance
    criteria are satisfied.

For each completed stage produce:

- implementation summary
- architecture explanation
- files added
- files modified
- syscalls/API changes
- security implications
- WovenGuard implications
- concurrency/SMP analysis
- tests added
- acceptance results
- known limitations
- next-stage prerequisites
- Git commit hash

The objective is not merely to make WovenHat boot.

The objective is to produce a secure, memory-safe, SMP-capable,
event-driven, graphical, AI-integrated, installable and maintainable
general-purpose operating system whose architecture can continue evolving
after WovenHat OS 1.0.
