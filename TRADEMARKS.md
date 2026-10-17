# Trademark policy

The code, firmware and hardware designs of DS2000 are free software under the
[GNU Affero General Public License v3.0](LICENSE). The license covers what the
project is made of. It does not cover its name or its logos, and this policy
says how you may use them.

The point is simple: someone who downloads, installs or buys something called
DS2000 should be getting the real thing, or know clearly that they are not.
Forks are welcome. They just need a name of their own.

This policy is also published at
<https://mechardo3d.xyz/en/ds2000/trademark>.

## The marks

- The name **DS2000**, used for the device, its app and its firmware.
- The **DS2000 logo**.
- The name **Mechardo Labs** and its logos, including the `mechardolabs`
  wordmark and the `m` mark.

They belong to Mechardo Labs. The logo files in this repository are covered by
this policy, not by the AGPL.

## Additional term under the AGPL

Under section 7(e) of the GNU Affero General Public License v3.0, the license of
this repository does not grant rights under trademark law to use the marks
above. Use of the marks is governed by this policy. The license text in
[`LICENSE`](LICENSE) is unchanged, and every other right it gives you stands.

## What you may do without asking

- **Talk about the project.** Name DS2000 in articles, videos, reviews,
  tutorials, talks and course material, and use the logo next to it, as long
  as it is clear who made what.
- **Redistribute official releases unchanged**, under their name.
- **Say what your work is for.** "Compatible with DS2000", "for DS2000",
  "based on DS2000" and "a fork of DS2000" are fine when they are true.
- **Build and sell devices from the unmodified designs** running the official
  firmware, and describe them as "built from the open-source DS2000 design".
  Say who built them, and do not present them as made, sold or supported by
  Mechardo Labs.
- **Fork the repositories to contribute.** A fork made to send changes back
  can keep the name while it is used for that.
- **Use the logo to link to the project**, unchanged.

## What needs written permission

- Calling a **modified** version of the app, the firmware or the hardware
  "DS2000", or a name that can be confused with it (for example "DS2000 Pro"
  or "DS-2000"). Modified includes different code, different firmware and a
  changed board or enclosure.
- Using the logos on, or as the icon of, a modified version or another product,
  or altering the logos.
- Saying or implying that Mechardo Labs made, endorses, certifies or supports
  something it did not.
- Publishing installers, firmware or devices under the name that were not built
  from the official sources, or signing them so they look like official
  releases.
- Using the marks in a domain name, an app store listing, a company or product
  name, or a social media account that could pass for the official one.

## Renaming a fork

A fork you publish under another name should change the name and the logos,
and also anything that would make it pass for, or update itself from, the
official project. What that means here is listed below.

### In this repository (desktop app)

Files covered by this policy: `docs/img/ds2000-logo.webp`,
`src-tauri/icons/` (`ds2000-logo.ico`, `ds2000-logo.png`, `icon.png`,
`favicon.ico`), `src-tauri/frontend/static/ds2000-logo.png` and
`src-tauri/frontend/static/Mechardo-labs.svg`.

A renamed fork changes:

- `productName`, `identifier` (`xyz.mechardo3d.ds2000`), `bundle.publisher`
  and `bundle.icon` in `src-tauri/tauri.conf.json`, and the window and tray
  titles.
- The updater: `plugins.updater.endpoints` and `plugins.updater.pubkey`, with
  its own signing key. Otherwise installed copies of the fork would ask the
  official project for updates.
- `FIRMWARE_PUBLIC_KEY` and the firmware repository in
  `src-tauri/src/controller/firmware.rs`, if the fork ships its own firmware.
- The links to mechardo3d.xyz, the Discord invite and the social accounts in
  the frontend.

## Asking

To ask for permission, or to report a use that looks wrong, write through
<https://mechardo3d.xyz/en/contact> or open an issue. Mechardo Labs may update
this policy; the current version is the one in the main branch.

This policy is not legal advice. Where it disagrees with the law that applies
to you, the law wins.
