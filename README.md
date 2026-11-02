# Tartarusolver

A screen overlay for Tartarus Prison, the Warden's Duty minigame of Torchlight: Infinite. While you play, it draws a
box around the best choice: where to go, Consume / Replace / Hunt / End, which Inmate to consume, which instrument,
which treasure. It aims for 10 Chromatics, then as many red (Ultimate) beasts on the board as possible. In the
bottom-left corner it shows the expected result of the boxed choice and a bar until the next box.

> **Not affiliated.** An independent fan project, not affiliated with, endorsed, sponsored or approved by the
> developers or publishers of Torchlight: Infinite. Torchlight: Infinite and its names and images belong to their
> respective owners.

## Purely an overlay

It only looks and shows. The boxes are drawn in a separate, see-through window on top of the game. It **never** sends
a click, a key press or any other input to the game, never plays or automates a move, and never reads or changes the
game's memory, files or network traffic. It reads two things: the game's picture on your screen, and the game's own
log file (`UE_game.log`). Every decision and every click is yours.

Whether a tool like this is allowed is set by the game's terms of service: check them yourself before use.

## Use

1. In the game's settings, turn on **Enable log**. The overlay follows the game through its log file (UE_game.log):
   without it, it can only guess from the screen, and misses or misreads some screens and the day.
2. Download `tartarus-overlay.exe` from the [latest release](../../releases/latest).
3. Start it, read the notice, click **I accept**, then play Tartarus Prison.

Its icon sits next to the clock (it may be under the "^" arrow): click it for **Boxes on/off**, **Settings...** and
**Quit**. Default hotkeys: Ctrl+Alt+B boxes on/off, Ctrl+Alt+R read the screen again, Ctrl+Alt+Q quit. It also quits
on its own a couple of minutes after the game closes. It works at any resolution and UI scale.

### Settings

From the icon, **Settings...**: the game log (found through Steam by default; **Browse...** for another install), the
three hotkeys, and the box color. They're kept in `%APPDATA%\TartarusSolver\settings.json`.

## FAQ

**Why do my fans spin up?** For about half a second after each choice it plays thousands of possible futures of the
run on all your processor's cores, at low priority so the game comes first. Then it rests.

**Does it send anything anywhere?** No. Nothing leaves your computer. It writes only `settings.json` (when you change
settings or tick "Don't show this again").

**Windows says it protected my PC / my antivirus warns.** The file isn't code-signed yet, so Windows doesn't know it.
Check that your copy is the original: in PowerShell, `Get-FileHash tartarus-overlay.exe` must show the SHA-256 listed
in the release's `SHA256SUMS.txt`. Only download it from this repository's releases.

**The boxes don't appear, or come late.** Check that **Enable log** is on in the game's settings, that the game log
was found (Settings... shows it), and that the game runs in a window or borderless window, not exclusive full screen.

**Is the advice always right?** No. It comes from a simulation of the rules as they are known; a good choice can still
end badly.

## Build

    cargo build --release

on Windows gives `target/release/tartarus-overlay.exe`.

## License and warranty

MIT (see [LICENSE](LICENSE)): provided **as is, without warranty of any kind**. The author is not liable for any loss
or damage, in the game or otherwise, nor for any action the game's publisher may take on your account. Use at your own
risk.
