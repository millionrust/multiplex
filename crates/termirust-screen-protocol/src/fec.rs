//! Forward error correction for the motion path: systematic Reed–Solomon over GF(256).
//!
//! A long-term reference only helps once the viewer has told the host something was lost, which
//! costs a round trip. Parity avoids most of those round trips: send a few extra shards with every
//! group of video frames, and a viewer that loses one rebuilds it without asking for anything.
//!
//! **Systematic** means the data shards go out unchanged — they are the ordinary
//! [`Message::VideoFrame`](crate::Message::VideoFrame)s — and the parity shards are extra. A
//! viewer that loses nothing ignores the parity entirely, so the only cost of being wrong about
//! the loss rate is bandwidth, never latency.
//!
//! The code is Reed–Solomon built on a **Cauchy** matrix, whose every square submatrix is
//! invertible. That is what makes the code maximum-distance separable: any `data` shards out of
//! the `data + parity` sent rebuild the group, whichever ones they happen to be.
//!
//! Written by hand rather than taken from a crate. It is about two hundred lines of finite-field
//! arithmetic with an exact property to test against — erase any `parity` shards and the group
//! must come back byte for byte — and it sits in the dependency closure of a security-reviewed
//! workspace, where a new package costs an ADR amendment.

use crate::ProtocolError;

/// A group can have at most this many shards in total, data and parity together. The field has
/// 256 elements and a Cauchy matrix needs a distinct one per row and per column.
pub const MAX_SHARDS: usize = 256;

/// The primitive polynomial for GF(2^8), the same one AES uses.
const POLY: u16 = 0x11D;

struct Field {
    /// `exp[i]` is the generator raised to `i`, doubled in length so a sum of two logarithms
    /// never needs a modulo.
    exp: [u8; 512],
    /// `log[v]` is the discrete logarithm of `v`. `log[0]` is meaningless and never read.
    log: [u8; 256],
}

const FIELD: Field = build_field();

const fn build_field() -> Field {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut value: u16 = 1;
    let mut power = 0;
    while power < 255 {
        exp[power] = value as u8;
        log[value as usize] = power as u8;
        value <<= 1;
        if value & 0x100 != 0 {
            value ^= POLY;
        }
        power += 1;
    }
    let mut at = 255;
    while at < 512 {
        exp[at] = exp[at - 255];
        at += 1;
    }
    Field { exp, log }
}

const fn multiply(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    FIELD.exp[FIELD.log[a as usize] as usize + FIELD.log[b as usize] as usize]
}

/// The multiplicative inverse. Zero has none, and is never asked for: a Cauchy entry is built
/// from two distinct field elements, so it is never zero.
const fn invert(a: u8) -> u8 {
    FIELD.exp[255 - FIELD.log[a as usize] as usize]
}

/// A Reed–Solomon code for a fixed shape: `data` shards in, `parity` extra shards out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fec {
    data: usize,
    parity: usize,
    /// `parity` rows of `data` entries, in row-major order.
    coefficients: Vec<u8>,
}

impl Fec {
    /// A code that repairs up to `parity` missing shards out of `data + parity`.
    pub fn new(data: usize, parity: usize) -> Result<Self, ProtocolError> {
        if data == 0 || parity == 0 || data + parity > MAX_SHARDS {
            return Err(ProtocolError::Malformed);
        }
        // Cauchy: entry [j][i] is 1 / (x_j + y_i), where addition is exclusive-or. Taking the
        // column elements from below `data` and the row elements from `data` upwards keeps the
        // two sets disjoint, so no entry is ever an inverse of zero.
        let mut coefficients = vec![0u8; parity * data];
        for row in 0..parity {
            for column in 0..data {
                let x = (data + row) as u8;
                let y = column as u8;
                coefficients[row * data + column] = invert(x ^ y);
            }
        }
        Ok(Self {
            data,
            parity,
            coefficients,
        })
    }

    pub const fn data_shards(&self) -> usize {
        self.data
    }

    pub const fn parity_shards(&self) -> usize {
        self.parity
    }

    /// The parity shards for one group. Every data shard must be the same length; use
    /// [`shard_length`] and [`pack_shard`] to get there.
    pub fn encode(&self, shards: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        if shards.len() != self.data {
            return Err(ProtocolError::Malformed);
        }
        let length = shards[0].len();
        if length == 0 || shards.iter().any(|shard| shard.len() != length) {
            return Err(ProtocolError::Malformed);
        }
        let mut out = vec![vec![0u8; length]; self.parity];
        for (row, parity) in out.iter_mut().enumerate() {
            for (column, shard) in shards.iter().enumerate() {
                let coefficient = self.coefficients[row * self.data + column];
                if coefficient == 0 {
                    continue;
                }
                for (destination, source) in parity.iter_mut().zip(shard) {
                    *destination ^= multiply(coefficient, *source);
                }
            }
        }
        Ok(out)
    }

    /// Rebuilds the missing data shards in place.
    ///
    /// `data` holds what arrived, with a `None` for each shard that did not; `parity` holds the
    /// parity shards that arrived, each with the index it was sent under. Fails when too much is
    /// missing, which is the case the viewer reports as lost.
    pub fn rebuild(
        &self,
        data: &mut [Option<Vec<u8>>],
        parity: &[(usize, Vec<u8>)],
    ) -> Result<(), ProtocolError> {
        if data.len() != self.data {
            return Err(ProtocolError::Malformed);
        }
        if data.iter().all(Option::is_some) {
            return Ok(());
        }
        let length = data
            .iter()
            .flatten()
            .chain(parity.iter().map(|(_, shard)| shard))
            .map(Vec::len)
            .next()
            .ok_or(ProtocolError::Malformed)?;
        if length == 0
            || data.iter().flatten().any(|shard| shard.len() != length)
            || parity.iter().any(|(_, shard)| shard.len() != length)
            || parity.iter().any(|(index, _)| *index >= self.parity)
        {
            return Err(ProtocolError::Malformed);
        }

        let missing: Vec<usize> = data
            .iter()
            .enumerate()
            .filter(|(_, shard)| shard.is_none())
            .map(|(at, _)| at)
            .collect();
        let rebuilt = self.solve(data, parity, &missing, length)?;
        for (at, shard) in missing.into_iter().zip(rebuilt) {
            data[at] = Some(shard);
        }
        Ok(())
    }

    /// Solves the group for the shards named in `missing`, from whatever arrived.
    fn solve(
        &self,
        data: &[Option<Vec<u8>>],
        parity: &[(usize, Vec<u8>)],
        missing: &[usize],
        length: usize,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        // One row per surviving shard, up to the `data` rows needed to solve for the group: a
        // unit row for a data shard that arrived, its Cauchy row for a parity shard.
        let mut matrix: Vec<u8> = Vec::with_capacity(self.data * self.data);
        let mut known: Vec<&[u8]> = Vec::with_capacity(self.data);
        for (column, shard) in data.iter().enumerate() {
            if known.len() == self.data {
                break;
            }
            if let Some(shard) = shard {
                let mut row = vec![0u8; self.data];
                row[column] = 1;
                matrix.extend_from_slice(&row);
                known.push(shard);
            }
        }
        for (index, shard) in parity {
            if known.len() == self.data {
                break;
            }
            matrix
                .extend_from_slice(&self.coefficients[index * self.data..(index + 1) * self.data]);
            known.push(shard);
        }
        if known.len() < self.data {
            // Fewer shards than the group has dimensions: nothing can rebuild it.
            return Err(ProtocolError::Malformed);
        }

        let inverse = invert_matrix(&matrix, self.data)?;
        Ok(missing
            .iter()
            .map(|row| {
                let mut rebuilt = vec![0u8; length];
                for (column, shard) in known.iter().enumerate() {
                    let coefficient = inverse[row * self.data + column];
                    if coefficient == 0 {
                        continue;
                    }
                    for (destination, source) in rebuilt.iter_mut().zip(shard.iter()) {
                        *destination ^= multiply(coefficient, *source);
                    }
                }
                rebuilt
            })
            .collect())
    }
}

/// Gauss–Jordan elimination in GF(256), returning the inverse in row-major order.
fn invert_matrix(matrix: &[u8], size: usize) -> Result<Vec<u8>, ProtocolError> {
    let mut work = matrix.to_vec();
    let mut inverse = vec![0u8; size * size];
    for at in 0..size {
        inverse[at * size + at] = 1;
    }
    for column in 0..size {
        // A Cauchy matrix cannot be singular, but the caller's shard set decides which rows are
        // here, so a zero pivot is refused rather than trusted away.
        let pivot = (column..size).find(|row| work[row * size + column] != 0);
        let Some(pivot) = pivot else {
            return Err(ProtocolError::Malformed);
        };
        if pivot != column {
            for at in 0..size {
                work.swap(pivot * size + at, column * size + at);
                inverse.swap(pivot * size + at, column * size + at);
            }
        }
        let scale = invert(work[column * size + column]);
        for at in 0..size {
            work[column * size + at] = multiply(work[column * size + at], scale);
            inverse[column * size + at] = multiply(inverse[column * size + at], scale);
        }
        for row in 0..size {
            if row == column {
                continue;
            }
            let factor = work[row * size + column];
            if factor == 0 {
                continue;
            }
            for at in 0..size {
                work[row * size + at] ^= multiply(factor, work[column * size + at]);
                inverse[row * size + at] ^= multiply(factor, inverse[column * size + at]);
            }
        }
    }
    Ok(inverse)
}

/// How long every shard of a group has to be: the longest payload plus its length prefix.
///
/// Reed–Solomon works across equal-length shards, and video frames are not equal length, so each
/// one carries its own length and is padded out to the group's.
pub fn shard_length(payloads: &[Vec<u8>]) -> usize {
    payloads.iter().map(Vec::len).max().unwrap_or(0) + 4
}

/// One payload as a shard of `length`: its length, then its bytes, then zeros.
pub fn pack_shard(payload: &[u8], length: usize) -> Result<Vec<u8>, ProtocolError> {
    if payload.len() + 4 > length || payload.len() > u32::MAX as usize {
        return Err(ProtocolError::Malformed);
    }
    let mut shard = Vec::with_capacity(length);
    shard.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    shard.extend_from_slice(payload);
    shard.resize(length, 0);
    Ok(shard)
}

/// The payload back out of a shard. A rebuilt shard whose length prefix is impossible means the
/// group was rebuilt from the wrong shards, so it is refused rather than passed to a decoder.
pub fn unpack_shard(shard: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    if shard.len() < 4 {
        return Err(ProtocolError::Malformed);
    }
    let length = u32::from_be_bytes(shard[..4].try_into().expect("four bytes")) as usize;
    if length == 0 || length + 4 > shard.len() {
        return Err(ProtocolError::Malformed);
    }
    Ok(shard[4..4 + length].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn group(count: usize, length: usize) -> Vec<Vec<u8>> {
        (0..count)
            .map(|index| {
                (0..length)
                    .map(|at| (index * 31 + at * 17 + 7) as u8)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_field_is_a_field() {
        for value in 1..=255u8 {
            assert_eq!(multiply(value, invert(value)), 1, "{value} has no inverse");
            assert_eq!(multiply(value, 1), value);
            assert_eq!(multiply(value, 0), 0);
        }
        // Multiplication is associative and distributes over exclusive-or, which is what makes
        // the elimination above valid.
        for a in [1u8, 3, 17, 200, 255] {
            for b in [1u8, 5, 19, 128, 254] {
                for c in [1u8, 7, 23, 64, 253] {
                    assert_eq!(multiply(multiply(a, b), c), multiply(a, multiply(b, c)));
                    assert_eq!(multiply(a, b ^ c), multiply(a, b) ^ multiply(a, c));
                }
            }
        }
    }

    #[test]
    fn any_two_losses_are_repaired_by_two_parity_shards() {
        let fec = Fec::new(4, 2).unwrap();
        let shards = group(4, 64);
        let parity = fec.encode(&shards).unwrap();
        assert_eq!(parity.len(), 2);

        for first in 0..4 {
            for second in 0..4 {
                let mut received: Vec<Option<Vec<u8>>> = shards.iter().cloned().map(Some).collect();
                received[first] = None;
                received[second] = None;
                let held: Vec<(usize, Vec<u8>)> = parity.iter().cloned().enumerate().collect();
                fec.rebuild(&mut received, &held).unwrap();
                assert_eq!(
                    received.into_iter().map(Option::unwrap).collect::<Vec<_>>(),
                    shards,
                    "losing {first} and {second}"
                );
            }
        }
    }

    #[test]
    fn a_lost_parity_shard_does_not_stop_the_repair() {
        let fec = Fec::new(4, 2).unwrap();
        let shards = group(4, 32);
        let parity = fec.encode(&shards).unwrap();
        let mut received: Vec<Option<Vec<u8>>> = shards.iter().cloned().map(Some).collect();
        received[2] = None;
        // Only the second parity shard survived, and it is enough on its own.
        fec.rebuild(&mut received, &[(1, parity[1].clone())])
            .unwrap();
        assert_eq!(received[2].as_ref(), Some(&shards[2]));
    }

    #[test]
    fn more_losses_than_parity_shards_cannot_be_repaired() {
        let fec = Fec::new(4, 1).unwrap();
        let shards = group(4, 16);
        let parity = fec.encode(&shards).unwrap();
        let mut received: Vec<Option<Vec<u8>>> = shards.iter().cloned().map(Some).collect();
        received[0] = None;
        received[3] = None;
        assert_eq!(
            fec.rebuild(&mut received, &[(0, parity[0].clone())]),
            Err(ProtocolError::Malformed),
            "two gone and one parity shard is a group the viewer has to report as lost"
        );
    }

    #[test]
    fn shapes_outside_the_field_are_refused() {
        assert!(Fec::new(0, 2).is_err());
        assert!(Fec::new(4, 0).is_err());
        assert!(
            Fec::new(200, 100).is_err(),
            "more shards than the field has"
        );
        assert!(Fec::new(255, 1).is_ok());
    }

    #[test]
    fn a_shard_carries_its_own_length() {
        let payloads = vec![vec![1u8, 2, 3], vec![9u8; 40], Vec::new()];
        let length = shard_length(&payloads);
        assert_eq!(length, 44);
        assert_eq!(
            unpack_shard(&pack_shard(&payloads[0], length).unwrap()).unwrap(),
            payloads[0]
        );
        assert!(
            pack_shard(&[0u8; 41], length).is_err(),
            "a payload that does not fit its group"
        );
        assert!(
            unpack_shard(&[0, 0, 0, 200, 1, 2]).is_err(),
            "a length past the end of the shard"
        );
        assert!(unpack_shard(&[0, 0]).is_err(), "no length at all");
    }

    proptest! {
        /// The property the whole design rests on: any `data` of the `data + parity` shards sent
        /// rebuild the group exactly, whichever ones survive.
        #[test]
        fn any_surviving_shards_rebuild_the_group(
            data in 1usize..7,
            parity in 1usize..5,
            length in 1usize..48,
            seed in any::<u64>(),
        ) {
            let fec = Fec::new(data, parity).unwrap();
            let shards = group(data, length);
            let encoded = fec.encode(&shards).unwrap();

            // Drop shards until exactly `data` are left, choosing which from the seed.
            let mut alive: Vec<bool> = vec![true; data + parity];
            let mut state = seed | 1;
            let mut to_drop = parity;
            while to_drop > 0 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let at = (state >> 33) as usize % alive.len();
                if alive[at] {
                    alive[at] = false;
                    to_drop -= 1;
                }
            }

            let mut received: Vec<Option<Vec<u8>>> = shards
                .iter()
                .enumerate()
                .map(|(at, shard)| alive[at].then(|| shard.clone()))
                .collect();
            let held: Vec<(usize, Vec<u8>)> = encoded
                .iter()
                .enumerate()
                .filter(|(at, _)| alive[data + at])
                .map(|(at, shard)| (at, shard.clone()))
                .collect();

            fec.rebuild(&mut received, &held).unwrap();
            prop_assert_eq!(
                received.into_iter().map(Option::unwrap).collect::<Vec<_>>(),
                shards
            );
        }

        #[test]
        fn rebuilding_from_nonsense_never_panics(
            bytes in proptest::collection::vec(any::<u8>(), 0..64),
            data in 1usize..5,
            parity in 1usize..4,
        ) {
            let fec = Fec::new(data, parity).unwrap();
            let mut received: Vec<Option<Vec<u8>>> = vec![None; data];
            if !bytes.is_empty() {
                received[0] = Some(bytes.clone());
            }
            let _ = fec.rebuild(&mut received, &[(0, bytes.clone())]);
            let _ = unpack_shard(&bytes);
        }
    }
}
