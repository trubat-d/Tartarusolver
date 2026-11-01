# Tartarusolver

A screen overlay for Tartarus Prison, the Warden's Duty minigame of Torchlight: Infinite. While you play, it draws a
green box around the best choice: where to go, Consume / Replace / End, which Inmate to consume, which instrument,
which treasure. It aims for 10 Chromatics, then as many red (Ultimate) beasts on the board as possible.

It only looks at the screen and the game's log. It never clicks, types or touches the game.

## Use

1. Download `tartarus-overlay.exe` from the latest release.
2. Start it, then play Tartarus Prison.

There is no console and it writes no files. Hotkeys:

- Ctrl+Alt+B: boxes on/off
- Ctrl+Alt+R: read the screen again
- Ctrl+Alt+Q: quit (it also quits on its own a couple of minutes after the game closes)

It works at any resolution and UI scale. Closing it forgets the current run, so start it before the run.

## Build

    cargo build --release

on Windows gives `target/release/tartarus-overlay.exe`.

## License

MIT
