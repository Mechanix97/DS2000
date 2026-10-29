# Changelog

What changes for someone using the DS2000 app, release by release. Each GitHub release starts
with its entry from this file, and the Discord announcement is built from it. The pull requests
behind every release are listed in its notes on GitHub.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.5.2] - 2026-10-29

Serial protocol 1: pairs with DS2000-Firmware 0.2.1.

### Fixed

- With **Start minimised** on, opening the app yourself now shows its window. Only the start
  with Windows goes straight to the tray, so the option can only be changed while **Start with
  Windows** is on.

## [0.5.1] - 2026-10-27

Serial protocol 1: pairs with DS2000-Firmware 0.2.1.

### Added

- **About → Device firmware → Changelog** shows what changed in each firmware release.

### Changed

- The release notes inside the app no longer open with the serial protocol line.
- **Update** on the app update notice installs the update, showing its progress in About. It
  used to only open About.

## [0.5.0] - 2026-10-27

Serial protocol 1: pairs with DS2000-Firmware 0.2.1.

### Added

- After an update, the app opens with what changed since the version you had. **About →
  Changelog** shows it again, for every version up to the one you have.
- Desktop notifications when the window is closed: the device unplugged, the connection to Discord
  lost, or Discord waiting for you to authorise DS2000, and another when the device or Discord is
  back. They can be turned off in Settings.

## [0.4.1] - 2026-10-23

Serial protocol 1: pairs with DS2000-Firmware 0.2.1.

### Fixed

- The app reconnects to Discord on its own a week after linking. Discord's sign-in expires after
  seven days, and the app used to stay disconnected until the credentials were entered again.

## [0.4.0] - 2026-10-21

Serial protocol 1: pairs with DS2000-Firmware 0.2.1.

### Changed

- New interface: a sidebar with the RGB, Discord, Settings and About sections, your Discord
  account at its bottom, a fixed-size window and a bundled font.
- Status notices above the account: problems stay until they are fixed, fixes show briefly, and
  updates come with an **Update** button. When Discord is not running it says "Discord closed".
- The RGB tab draws the device, with its LEDs and keys lighting up as the real ones do, and has a
  hex field and a colour wheel for each LED.
- The Discord tab walks through the setup in three steps, each marked when done. A saved secret
  shows as saved, and unlinking asks first.
- Links to mechardo3d.xyz open in the app's language.
- App updates are checked at start and once a day, firmware updates when the device connects.

### Fixed

- The RGB tab opens on the saved lighting. It used to show the defaults, and the first change
  overwrote what was saved.

## [0.3.0] - 2026-10-17

Serial protocol 1: pairs with DS2000-Firmware 0.2.1.

### Added

- **About → Check for firmware updates** installs the latest firmware for your board. The image is
  checked against its checksum and signature before it is written.
- Logs are written to a file, 5 MB at most, and **About → Open logs folder** opens it, to attach
  to a bug report.

### Changed

- The DS2000 name and logos are covered by a [trademark policy](TRADEMARKS.md). Forks are welcome
  under a name of their own.

## [0.2.0] - 2026-10-17

First public release. Serial protocol 1: pairs with DS2000-Firmware 0.2.0.

### Added

- Mute, deafen and leave the voice channel from the device keys or from the app, through Discord's
  local RPC and a Discord application of your own.
- The device LEDs follow your voice state. RGB modes rainbow, fixed and breathing, with brightness
  and speed.
- Runs in the tray, can start with Windows and start minimised, and only one copy runs at a time.
- English and Spanish interface.
- Only DS2000 devices are opened, and the app and the device check they speak the same protocol.
- A Windows installer on every release, with its SHA-256.
- **About → Check for updates** installs newer releases.

[Unreleased]: https://github.com/Mechanix97/DS2000/compare/v0.5.2...HEAD
[0.5.2]: https://github.com/Mechanix97/DS2000/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/Mechanix97/DS2000/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/Mechanix97/DS2000/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/Mechanix97/DS2000/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/Mechanix97/DS2000/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Mechanix97/DS2000/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Mechanix97/DS2000/releases/tag/v0.2.0
