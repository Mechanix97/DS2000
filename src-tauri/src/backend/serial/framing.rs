//! Wire framing: COBS with a CRC-16, delimited by `0x00`.
//!
//! A frame on the wire is `COBS(body ++ crc16(body)) ++ 0x00`, where `body` is the message code
//! followed by its payload and the CRC is big-endian.
//!
//! - COBS removes every `0x00` from the encoded bytes, so the delimiter can never appear inside a
//!   frame and every payload byte may take the full 0-255 range. The previous framing ended
//!   frames on an unescaped `0xFF`, which made 255 unreachable for brightness and colour.
//! - The CRC is what turns a lost or corrupted byte into a rejected frame. Before it, dropping the
//!   first byte of a mute press (`02 00`) left `00`, a perfectly valid ping.
//!
//! Both are implemented here rather than pulled in as crates: together they are a few dozen lines,
//! and the firmware carries the same code, so keeping the two side by side is easier to audit.

use crate::error::SerialMessageError;

/// Ends every frame. COBS guarantees it appears nowhere else.
pub const FRAME_DELIMITER: u8 = 0x00;

/// Bytes the CRC adds to a body.
const CRC_LENGTH: usize = 2;

/// CRC-16/CCITT-FALSE: polynomial 0x1021, initial value 0xFFFF, no reflection, no final XOR.
///
/// Bitwise rather than table-driven: frames are a dozen bytes, and the firmware computes the same
/// thing on a microcontroller where a 512-byte table is not free.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Encodes `data` with COBS, appending the result to `out`. The delimiter is not added.
pub fn cobs_encode(data: &[u8], out: &mut Vec<u8>) {
    // Index of the code byte for the block being built, patched once the block ends.
    let mut code_index = out.len();
    out.push(0);
    let mut code: u8 = 1;

    for &byte in data {
        if byte == 0 {
            out[code_index] = code;
            code_index = out.len();
            out.push(0);
            code = 1;
            continue;
        }
        out.push(byte);
        code += 1;
        if code == 0xFF {
            // A full block of 254 non-zero bytes ends without an implied zero.
            out[code_index] = code;
            code_index = out.len();
            out.push(0);
            code = 1;
        }
    }
    out[code_index] = code;
}

/// Decodes one COBS-encoded frame, without its delimiter.
pub fn cobs_decode(encoded: &[u8]) -> Result<Vec<u8>, SerialMessageError> {
    let mut out = Vec::with_capacity(encoded.len());
    let mut index = 0;

    while index < encoded.len() {
        let code = encoded[index];
        if code == 0 {
            return Err(SerialMessageError::InvalidFraming);
        }
        let block_end = index + usize::from(code);
        if block_end > encoded.len() {
            // The block claims more bytes than the frame holds: something was lost.
            return Err(SerialMessageError::InvalidFraming);
        }
        out.extend_from_slice(&encoded[index + 1..block_end]);
        index = block_end;
        // Every block but a full one implies a zero, except at the very end of the frame.
        if code != 0xFF && index < encoded.len() {
            out.push(0);
        }
    }
    Ok(out)
}

/// Builds the wire bytes for one message body: CRC, COBS and the delimiter.
pub fn encode_frame(body: &[u8], out: &mut Vec<u8>) {
    let mut checked = Vec::with_capacity(body.len() + CRC_LENGTH);
    checked.extend_from_slice(body);
    checked.extend_from_slice(&crc16(body).to_be_bytes());
    cobs_encode(&checked, out);
    out.push(FRAME_DELIMITER);
}

/// Recovers a message body from one frame's bytes, delimiter excluded.
///
/// Fails when the COBS encoding is broken or the CRC does not match, which is what a lost,
/// added or flipped byte looks like.
pub fn decode_frame(encoded: &[u8]) -> Result<Vec<u8>, SerialMessageError> {
    let mut checked = cobs_decode(encoded)?;
    if checked.len() <= CRC_LENGTH {
        // A body needs at least its message code.
        return Err(SerialMessageError::InvalidMessageLength);
    }
    let body_length = checked.len() - CRC_LENGTH;
    let received = u16::from_be_bytes([checked[body_length], checked[body_length + 1]]);
    checked.truncate(body_length);
    if crc16(&checked) != received {
        return Err(SerialMessageError::ChecksumMismatch);
    }
    Ok(checked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cobs(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        cobs_encode(data, &mut out);
        out
    }

    #[test]
    fn the_crc_matches_the_published_check_value() {
        // The standard check input for every CRC catalogue entry. The firmware tests against the
        // same value, which is what keeps the two implementations in agreement.
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    #[test]
    fn cobs_matches_the_reference_examples() {
        // From the examples in Cheshire and Baker's paper and the Wikipedia article.
        assert_eq!(cobs(&[0x00]), [0x01, 0x01]);
        assert_eq!(cobs(&[0x00, 0x00]), [0x01, 0x01, 0x01]);
        assert_eq!(
            cobs(&[0x11, 0x22, 0x00, 0x33]),
            [0x03, 0x11, 0x22, 0x02, 0x33]
        );
        assert_eq!(
            cobs(&[0x11, 0x00, 0x00, 0x00]),
            [0x02, 0x11, 0x01, 0x01, 0x01]
        );
    }

    #[test]
    fn cobs_round_trips_every_byte_value_and_long_runs() {
        let every_byte: Vec<u8> = (0..=255).collect();
        let long_run = vec![0xAB; 600];
        let mut ends_at_full_block = vec![0x01; 254];
        ends_at_full_block.push(0x00);

        for data in [every_byte, long_run, ends_at_full_block, Vec::new()] {
            let encoded = cobs(&data);
            assert!(!encoded.contains(&FRAME_DELIMITER), "delimiter leaked");
            assert_eq!(cobs_decode(&encoded).expect("decodes"), data);
        }
    }

    #[test]
    fn a_frame_carries_255_and_zero_through_intact() {
        let body = [0x04, 0xFF, 0x01, 0xFF, 0x00, 0xFF, 0x00];
        let mut wire = Vec::new();
        encode_frame(&body, &mut wire);

        assert_eq!(wire.last(), Some(&FRAME_DELIMITER));
        assert_eq!(
            wire.iter().filter(|&&byte| byte == FRAME_DELIMITER).count(),
            1
        );
        assert_eq!(
            decode_frame(&wire[..wire.len() - 1]).expect("decodes"),
            body
        );
    }

    #[test]
    fn a_lost_or_flipped_byte_is_rejected() {
        // The mute press from #25: losing its first byte used to leave a valid ping.
        let mut wire = Vec::new();
        encode_frame(&[0x02, 0x00], &mut wire);
        let frame = &wire[..wire.len() - 1];

        for missing in 0..frame.len() {
            let mut damaged = frame.to_vec();
            damaged.remove(missing);
            assert!(decode_frame(&damaged).is_err(), "byte {missing} dropped");
        }
        for flipped in 0..frame.len() {
            let mut damaged = frame.to_vec();
            damaged[flipped] ^= 0x10;
            assert!(decode_frame(&damaged).is_err(), "byte {flipped} flipped");
        }
    }

    #[test]
    fn a_frame_with_nothing_but_a_crc_is_rejected() {
        let mut wire = Vec::new();
        cobs_encode(&crc16(&[]).to_be_bytes(), &mut wire);

        assert_eq!(
            decode_frame(&wire),
            Err(SerialMessageError::InvalidMessageLength)
        );
    }
}
