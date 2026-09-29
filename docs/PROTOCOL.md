# DS2000 serial protocol

This document is the contract between the desktop application (this repository) and the device
firmware ([DS2000-Firmware](https://github.com/Mechanix97/DS2000-Firmware)). Both sides implement
it independently; when they disagree, this document is what is right.

**Current revision: protocol 1.**

| | Application | Firmware |
|---|---|---|
| Framing | `src-tauri/src/backend/serial/framing.rs` | `lib/protocol/` |
| Messages | `src-tauri/src/backend/serial/messages/` | `src/main.cpp` (`dispatch`) |
| Protocol version | `PROTOCOL_VERSION` in `serial_message.rs` | `PROTOCOL_VERSION` in `main.cpp` |

## Transport

USB CDC serial, 115200 baud, 8N1. The baud rate is nominal: on USB CDC it has no effect on speed.
The application raises DTR and RTS when it opens the port.

## Framing

Every message travels as one frame:

```
COBS( body ++ crc_hi ++ crc_lo ) ++ 0x00
```

- **body** is the message code followed by its payload.
- **CRC** is CRC-16/CCITT-FALSE over the body: polynomial `0x1021`, initial value `0xFFFF`, no
  reflection, no final XOR, sent big-endian. The check value for `"123456789"` is `0x29B1`.
- **COBS** (Consistent Overhead Byte Stuffing) removes every `0x00` from the encoded bytes, so
  `0x00` can only ever be the delimiter. Payload bytes take the full range 0-255.

A receiver:

1. collects bytes up to a `0x00`;
2. skips the frame if it is empty (back-to-back delimiters are allowed and carry nothing);
3. COBS-decodes it, and drops it if the encoding is broken;
4. drops it if it is shorter than 3 bytes (a code and the CRC), or if the CRC does not match;
5. dispatches the body.

A dropped frame is never reported back and never ends the connection. Receivers bound the frame
length: the firmware accepts bodies of up to 24 bytes and discards anything longer up to the next
delimiter. The application discards a run of more than 64 bytes without a delimiter.

## Messages

| Code | Name | Direction | Payload |
|---|---|---|---|
| `0x00` | [Hello](#hello-0x00) | app → device | none |
| `0x01` | [DeviceInfo](#deviceinfo-0x01) | device → app | 5 bytes |
| `0x02` | [Button](#button-0x02) | device → app | 1 byte |
| `0x03` | [VoiceSettings](#voicesettings-0x03) | app → device | 2 bytes |
| `0x04` | [RGB](#rgb-0x04) | app → device | 3 or 9 bytes |
| `0x05` | [RebootToBootloader](#reboottobootloader-0x05) | app → device | 4 bytes |

A receiver ignores codes it does not know and codes that only travel the other way.

### Hello (`0x00`)

Opens a session. No payload; a Hello with a payload is ignored. The device answers with
DeviceInfo.

### DeviceInfo (`0x01`)

| Byte | Field | |
|---|---|---|
| 0 | protocol | This document's revision the firmware implements |
| 1 | board | [Board id](#board-ids) the firmware was built for |
| 2 | major | Firmware version |
| 3 | minor | |
| 4 | patch | |

The protocol byte comes first and must stay first in every future revision, so that any two
revisions can at least tell each other apart.

#### Board ids

| Id | Board | Firmware environment |
|---|---|---|
| `0x01` | Waveshare RP2350-Zero | `rp2350_zero` |
| `0x02` | Raspberry Pi Pico 2 | `pico2` |
| `0x03` | Raspberry Pi Pico (RP2040) | `pico` |
| `0x10` | DS2000 rev A | (reserved, no environment yet) |

`0x00` is never assigned. The firmware sets the id per environment with `custom_board_id` in
`platformio.ini`; add new boards to this table and to `Board` in `device_info.rs`. The application
shows an unknown id rather than rejecting it.

### Button (`0x02`)

One byte: `0x00` mute, `0x01` deafen, `0x02` disconnect. Sent on each debounced press.

The device toggles its own mute/deafen LEDs immediately so they follow the key even with no
application running. The application then asks Discord for the change and sends VoiceSettings back
with the result, which confirms or corrects the LEDs.

### VoiceSettings (`0x03`)

| Byte | Field | |
|---|---|---|
| 0 | mute | `0x00` off, anything else on |
| 1 | deafen | `0x00` off, anything else on |

The mute LED lights while either is on, since deafening implies muting in Discord.

### RGB (`0x04`)

| Byte | Field | |
|---|---|---|
| 0 | brightness | 0-255 |
| 1 | mode | `0x00` rainbow, `0x01` fixed, `0x02` breathing |
| 2 | speed | 0-255, higher is faster. Ignored by fixed, sent anyway so its offset never moves |
| 3-5 | LED 1 | red, green, blue. Fixed and breathing only |
| 6-8 | LED 2 | red, green, blue. Fixed and breathing only |

Rainbow sends 3 bytes and chooses its own colours; fixed and breathing send 9. The device drops the
whole message, applying nothing, if the mode is unknown or the colours are missing.

### RebootToBootloader (`0x05`)

Payload is exactly the ASCII bytes `B O O T` (`42 4F 4F 54`); any other payload is ignored. The
device turns its LEDs off and reboots into USB boot, where it mounts as a drive that takes a UF2
image. It does not answer: its serial port disappearing is the reply.

## Handshake

The application runs this on every port it tries, before using it:

1. Send **Hello** and wait 200 ms for a frame.
2. **DeviceInfo** arrives: if its protocol equals the application's, the device is connected.
   Otherwise the port is closed and the device is reported as incompatible: *firmware out of date*
   if its protocol is lower, *application out of date* if higher. Nothing else is exchanged with it.
3. **Nothing arrives:** send the [legacy probe](#firmware-before-protocol-1) and wait 200 ms more.
   If it is answered, the device is reported as firmware out of date. Otherwise the port is not a
   DS2000.

An incompatible device keeps being retried with the reconnect backoff, so a device flashed with
matching firmware is picked up without restarting the application.

### Firmware before protocol 1

Earlier firmware ended frames with an unescaped `0xFF`, answered a ping `00 FF` with an empty pong
`01 FF`, and had no checksum or version. It cannot parse a COBS frame.

The legacy probe is the three bytes `FF 00 FF`: the first `0xFF` ends whatever the COBS Hello left
in the old firmware's buffer, and `00 FF` is its ping. That leftover is harmless: the Hello is
always `01 03 E1 F0 00`, which the old firmware reads as a pong and ignores. Protocol 0 is reported
for a device that answers the probe.

## Reference frames

Both implementations assert these bytes in their tests (`serial_message.rs`,
`test/test_protocol/test_protocol.cpp`):

| Message | Body | Wire |
|---|---|---|
| Hello | `00` | `01 03 E1 F0 00` |
| DeviceInfo: protocol 1, RP2350-Zero, 0.2.0 | `01 01 01 00 02 00` | `04 01 01 01 02 02 03 F1 37 00` |
| Button: mute | `02 00` | `02 02 03 7B 6D 00` |
| RebootToBootloader | `05 42 4F 4F 54` | `08 05 42 4F 4F 54 87 B0 00` |

## Changing the protocol

Any change to the framing, a message code, a payload layout or the meaning of a value is a new
revision:

1. Update this document first, including the reference frames.
2. Bump `PROTOCOL_VERSION` in both repositories.
3. Change both implementations and their reference-frame tests.
4. Open the two pull requests together, link each from the other, and merge them together.

Keep the protocol byte first in DeviceInfo and the Hello unchanged, so that the handshake still
reports a mismatch between any two revisions instead of seeing no device.

## History

| Protocol | Changes |
|---|---|
| 0 | Unversioned. Frames ended by a bare `0xFF`, so 255 could not be sent; no checksum; empty ping/pong handshake. |
| 1 | COBS + CRC-16 framing; Hello / DeviceInfo handshake with protocol, board and firmware version; RebootToBootloader. |
