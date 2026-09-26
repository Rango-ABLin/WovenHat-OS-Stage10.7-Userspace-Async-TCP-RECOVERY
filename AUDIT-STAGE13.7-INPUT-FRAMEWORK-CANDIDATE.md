# Stage 13.7 — WovenInput Candidate

Baseline: `864f94c` (accepted Stage 13.6 USB HID).

This candidate introduces the device-independent WovenInput event layer and routes the existing PS/2 keyboard path through it while preserving the foreground-terminal single-consumer rule.

Implemented:
- bounded unified input event queue
- device-independent keyboard, pointer, touch, pen and game-controller event types
- PS/2 scancode decoder publishes logical key events into WovenInput
- existing kernel shell/userspace stdin APIs remain compatible
- deterministic queue/self-test coverage
- Stage 13.7 1/2/4 CPU acceptance harness

Scope note: this is the Stage 13.7 framework foundation. USB HID report producers remain in the Stage 13.6 xHCI layer until their runtime service path is moved into the common driver/event pipeline; mouse/touchpad/touchscreen/pen/game-controller event types are defined now so later drivers target WovenInput rather than UI consumers directly.
