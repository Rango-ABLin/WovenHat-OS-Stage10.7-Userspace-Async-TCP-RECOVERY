# Stage 13.8 — WovenAudio / Intel HDA Foundation

Baseline: `ec91086` (accepted Stage 13.7 WovenInput).

Implemented in this candidate:
- WovenAudio device abstraction
- new `DeviceKind::Audio`
- PCI class/subclass discovery for High Definition Audio controllers
- MMIO BAR validation
- PCI memory decoding + bus mastering enablement
- HDA GCAP parsing for input/output/bidirectional stream counts and 64-bit DMA capability
- controller reset handshake through GCTL.CRST
- WovenDriver registration/binding for `hda0`
- bounded, lock-serialized controller ownership
- isolated Stage 13.8 acceptance on 1/2/4 CPUs

Scope:
This candidate proves the controller and subsystem foundation. It does not yet claim audible PCM playback, recording, codec topology discovery, mixer/volume control, DMA BDL programming, or userspace streams. Those belong to the next Stage 13.8 slices after controller initialization is accepted.

## Codec command transport slice
- 256-entry CORB/RIRB DMA rings
- ring-size validation and CORB read-pointer reset
- HDA verb transport with codec-address response validation
- STATESTS codec discovery
- Get Parameter vendor/revision/root-node discovery
- acceptance requires a real codec response

### CORB/RIRB polling correction
The first codec response proved DMA transport was active, but RINTCNT=1 caused the
controller/emulator to stop consuming further CORB commands once one response had
been produced. The polling bootstrap transport now uses a 255-response window.
This keeps interrupts disabled while allowing bounded multi-verb codec discovery.
A later interrupt-driven audio transport should explicitly service/clear RIRB
status and reset response accounting.

## Codec/widget topology slice
- discovers Audio Function Group nodes through Get Parameter(Function Group Type)
- discovers widget ranges through Get Parameter(Subordinate Node Count)
- classifies widget capabilities into DAC/audio-output, ADC/audio-input, mixer,
  selector, pin-complex, power and volume-knob categories
- rejects empty/meaningless codec topology
- Stage 13.8 acceptance requires non-empty HDA topology on 1/2/4 CPUs

## PCM DMA playback slice
- selects the first generic HDA Audio Output Converter discovered from codec topology
- allocates DMA-visible PCM and Buffer Descriptor List pages
- programs one output stream descriptor after the input-stream descriptor range
- uses 48 kHz / 16-bit / stereo bootstrap PCM format
- assigns stream tag 1 to the converter
- starts the HDA output DMA engine and verifies LPIB position advancement
- acceptance requires real DMA consumption; audible host output is not required
