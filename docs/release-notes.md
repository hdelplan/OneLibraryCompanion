# OneLibraryCompanion 0.4.0

Changes since the published Mac/Raspberry Pi 0.3.0 preview.

## Flexible display sizing

- Automatic sizing follows the available window or fullscreen space. Presets offer 1280 × 800, 1920 × 720, 1920 × 1080, 1280 × 720 and 1024 × 768, preserving proportions while fitting the window.
- Extra-wide screens support side-by-side decks or the existing stacked layout. Browse, MENU and Set History adapt to the available space.
- Waveform canvas resolution follows the actual display scale and refreshes after window resizing.

## Deck comparison

- Above each main waveform in the extra-wide side-by-side layout, switch between beat phase and the other deck's reference waveform by tapping the comparison header or changing MENU → Display.
- The phase meter shows orange/cyan lanes, white beat ticks, red downbeats and the master player. Each lane follows its own beat grid and tempo.
- Comparisons show unavailable when timing is stale, unreliable or under manual control. These displays do not establish hardware-validated phase accuracy.

## Track information

- My Tags appear as individual chips near the top of the CDJ track-information panel, with category tooltips.
- Long metadata is fully readable instead of being limited to two lines.
- The panel fits the available screen height and scrolls when needed.

## Reliability and diagnostics

- Temporary Browse source-refresh errors clear after a successful retry, with a clearer reconnecting message.
- Explicitly configured diagnostic hosts can record a one-minute bug report covering CDJ timing, browser errors and waveform performance. Browser capture continues when leaving MENU, retries failed uploads and offers a local download fallback. Reports remain on the configured host; recording is disabled by default.

## Downloads and setup

Apple Silicon and Intel Mac downloads include DMG and ZIP packages. Raspberry Pi downloads include ARM64 host and desktop Debian packages. Mac requires macOS 13+; Pi requires 64-bit Raspberry Pi OS Bookworm or later. Mac packages are ad-hoc signed and not notarized. Reinstall **Local USB Support** after updating the Mac app.

Stop players using OLC-served tracks before updating packages or restarting the host; loaded tracks may require reloading afterward.

[Installation and headless setup](distribution.md) · [Display settings](configuration.md#display) · [Compatibility and limits](compatibility.md).
