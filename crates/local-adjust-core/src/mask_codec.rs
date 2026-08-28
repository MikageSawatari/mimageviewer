//! Compact serialization for the per-pixel mask buffers.
//!
//! A mask carries one value per source pixel, so a 3082x4486 page holds 13.8M of
//! them. Serialized as a JSON number array that is 290 MB for a single page — the
//! shape that made one user's folder sidecar grow to 700 MB and block the UI for
//! six seconds on every folder switch.
//!
//! The buffers are stored instead as `"<tag>:<count>:<base64 of deflate(bytes)>"`:
//!
//! | field | tag | bytes |
//! | --- | --- | --- |
//! | alpha coverage (`f32` in 0..=1) | `q8z` | one `u8` per pixel, `round(a * 255)` |
//! | region labels (`u32` ids) | `u32z` | four little-endian bytes per pixel |
//!
//! On the real data above that is 290 MB -> 17.5 KB, because a mask is mostly one
//! flat value and deflate collapses the runs. Measured, not estimated.
//!
//! `<count>` is the element count. deflate accepts a truncated stream and simply
//! stops, so without it a half-written file would decode to a short buffer that
//! looks valid here and goes wrong much later, wherever it gets indexed.
//!
//! ## Why 8 bits for alpha
//!
//! The brush writes hard 0.0 / 1.0, so 1 bit would have been lossless for that
//! one source — but [`crate::SubjectMask`] holds the output of a matting model,
//! which is genuinely continuous. One encoding has to serve both, so it has to be
//! the one that keeps soft edges. 1/255 is finer than a mask can be seen at, and
//! deflate makes the binary case cost the same either way.
//!
//! ## Reading old data
//!
//! Local adjustment layers shipped in v1.1.0, so the number-array form exists in
//! released `local_adjust.db`, sidecar, and bundle files. Every decoder here
//! accepts **both** forms; only the encoder is new. Legacy decodes bump
//! [`legacy_decode_count`] so a caller can rewrite the file it just read.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use serde::de::{Error as DeError, SeqAccess, Visitor};
use serde::{Deserializer, Serializer};

/// Tag for 8-bit quantized coverage.
const TAG_ALPHA_U8: &str = "q8z:";
/// Tag for little-endian `u32` labels.
const TAG_LABEL_U32: &str = "u32z:";

/// Most entries a single mask may declare, one per source pixel.
///
/// The header is the first thing read from a file that may be corrupt, or that a
/// stranger wrote: edit bundles are made to be exchanged. So the count decides an
/// allocation before any of the data behind it has been seen, and every entry is
/// widened to four bytes once decoded.
///
/// 128 Mi entries is 134 megapixels, past any camera whose output can be edited
/// here — the largest images mIV opens are panoramas, and 360 view refuses to
/// enter edit mode at all, so no mask exists at that size. A page like the one in
/// the table above is 13.8M, so this leaves an order of magnitude in hand.
const MAX_MASK_ELEMENTS: usize = 128 * 1024 * 1024;

/// How much mask memory one document may decode, in bytes, unless a caller says less.
///
/// The largest real sidecar measured held 152,002,937 alpha values across every layer of
/// every item, which is 608 MB once widened to `f32`. A gigabyte clears that and still
/// bounds a file built to exhaust memory.
pub const DEFAULT_DOCUMENT_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;

thread_local! {
    /// Bytes left in the document being parsed on this thread. `None` means no document
    /// scope is open, in which case only the per-mask ceiling applies.
    static DOCUMENT_BUDGET: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// A budget covering every mask decoded until this value is dropped.
///
/// [`MAX_MASK_ELEMENTS`] bounds one field, which does not bound a file. A single
/// [`crate::LocalAdjustmentLayer`] can carry a subject mask, its source alpha, and both
/// manual-override rasters; a sidecar carries any number of layers over any number of
/// items. Every one of those can sit under the per-mask ceiling while the file as a whole
/// asks for tens of gigabytes — a flat alpha plane compresses to almost nothing, so the
/// file that does it is small.
///
/// Open one around each file parsed from disk. Charges are made from the count in the
/// header, before anything is decompressed, so an over-budget document is refused rather
/// than allocated.
#[must_use = "the budget applies only while this value is alive"]
pub struct DocumentBudget {
    previous: Option<u64>,
}

impl DocumentBudget {
    /// Open a scope with [`DEFAULT_DOCUMENT_BUDGET_BYTES`].
    pub fn open() -> Self {
        Self::with_bytes(DEFAULT_DOCUMENT_BUDGET_BYTES)
    }

    pub fn with_bytes(bytes: u64) -> Self {
        let previous = DOCUMENT_BUDGET.with(|budget| budget.replace(Some(bytes)));
        Self { previous }
    }

    /// Bytes still available in the open scope, for tests and diagnostics.
    #[must_use]
    pub fn remaining() -> Option<u64> {
        DOCUMENT_BUDGET.with(std::cell::Cell::get)
    }
}

impl Drop for DocumentBudget {
    fn drop(&mut self) {
        DOCUMENT_BUDGET.with(|budget| budget.set(self.previous));
    }
}

/// Charge one mask against the open document scope, if there is one.
///
/// Every element ends up four bytes wide in memory — `f32` coverage or a `u32` label —
/// so that is what a mask costs whichever codec reads it.
fn charge_document_budget<E: DeError>(count: usize) -> Result<(), E> {
    const BYTES_PER_ELEMENT_IN_MEMORY: u64 = 4;
    let cost = (count as u64).saturating_mul(BYTES_PER_ELEMENT_IN_MEMORY);
    DOCUMENT_BUDGET.with(|budget| match budget.get() {
        None => Ok(()),
        Some(remaining) if cost <= remaining => {
            budget.set(Some(remaining - cost));
            Ok(())
        }
        Some(remaining) => Err(E::custom(format!(
            "this document's masks decode to more than it may hold: another {cost} bytes              with {remaining} left"
        ))),
    })
}

static LEGACY_DECODES: AtomicU64 = AtomicU64::new(0);

/// How many buffers have been read from the old number-array form since the
/// process started.
///
/// Take it before and after a parse: if it moved, that file still holds the old
/// encoding and is worth rewriting. A concurrent parse on another thread can
/// inflate the difference, which costs at most one redundant rewrite.
pub fn legacy_decode_count() -> u64 {
    LEGACY_DECODES.load(Ordering::Relaxed)
}

fn note_legacy_decode() {
    LEGACY_DECODES.fetch_add(1, Ordering::Relaxed);
}

/// `count` is the element count the decoder must find again.
fn encode(tag: &str, count: usize, bytes: &[u8]) -> Option<String> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    // The encoder writes into a `Vec`, which does not fail. If that ever changes,
    // refuse to produce a value rather than fall back to a different encoding.
    let compressed = encoder
        .write_all(bytes)
        .and_then(|()| encoder.finish())
        .ok()?;
    let mut out = String::with_capacity(tag.len() + 24 + compressed.len() * 4 / 3);
    out.push_str(tag);
    out.push_str(&count.to_string());
    out.push(':');
    base64::engine::general_purpose::STANDARD.encode_string(&compressed, &mut out);
    Some(out)
}

/// Returns exactly `count * bytes_per_element` bytes, or an error.
fn decode<E: DeError>(tag: &str, bytes_per_element: usize, text: &str) -> Result<Vec<u8>, E> {
    let Some(rest) = text.strip_prefix(tag) else {
        let head: String = text.chars().take(8).collect();
        return Err(E::custom(format!(
            "unknown mask encoding {head:?} (expected {tag:?})"
        )));
    };
    let Some((count, payload)) = rest.split_once(':') else {
        return Err(E::custom("mask payload has no element count"));
    };
    let count: usize = count
        .parse()
        .map_err(|_| E::custom(format!("mask element count {count:?} is not a number")))?;
    if count > MAX_MASK_ELEMENTS {
        return Err(E::custom(format!(
            "mask declares {count} entries, past the {MAX_MASK_ELEMENTS} a mask may hold"
        )));
    }
    charge_document_budget::<E>(count)?;
    let expected = count
        .checked_mul(bytes_per_element)
        .ok_or_else(|| E::custom("mask buffer is implausibly large"))?;
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(payload.as_bytes())
        .map_err(|error| E::custom(format!("mask base64: {error}")))?;
    // Grow into the bytes that actually arrive rather than reserving what the header
    // asked for. `with_capacity(expected)` handed a corrupt count the whole allocation
    // before reading a byte, so a payload of a few kilobytes could take gigabytes and
    // end the process. `take` still stops a bomb one byte past the declared length.
    let mut out = Vec::new();
    DeflateDecoder::new(compressed.as_slice())
        .take(expected as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|error| E::custom(format!("mask deflate: {error}")))?;
    if out.len() != expected {
        return Err(E::custom(format!(
            "mask buffer decoded to {} bytes, but the header says {expected}",
            out.len()
        )));
    }
    Ok(out)
}

fn encoding_failed<E: serde::ser::Error>() -> E {
    E::custom("could not deflate the mask buffer")
}

// ── alpha coverage (`Vec<f32>` in 0..=1) ──────────────────────────────────

fn encode_alpha(alpha: &[f32]) -> Option<String> {
    let bytes: Vec<u8> = alpha
        .iter()
        .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    encode(TAG_ALPHA_U8, bytes.len(), &bytes)
}

struct AlphaVisitor;

impl<'de> Visitor<'de> for AlphaVisitor {
    type Value = Vec<f32>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a packed mask string or an array of numbers")
    }

    fn visit_str<E: DeError>(self, text: &str) -> Result<Self::Value, E> {
        let bytes = decode::<E>(TAG_ALPHA_U8, 1, text)?;
        Ok(bytes
            .into_iter()
            .map(|byte| f32::from(byte) / 255.0)
            .collect())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        read_legacy_seq(seq)
    }
}

/// Read the released number-array form under the same limits as the packed one.
///
/// The old form is verbose, so a file large enough to exhaust memory would itself be
/// enormous — but it decodes into the same buffers, and a document budget that only
/// counted one of the two encodings would be a hole a re-encode could walk through.
fn read_legacy_seq<'de, T, A>(mut seq: A) -> Result<Vec<T>, A::Error>
where
    T: serde::Deserialize<'de>,
    A: SeqAccess<'de>,
{
    note_legacy_decode();
    let mut out: Vec<T> = Vec::new();
    while let Some(value) = seq.next_element::<T>()? {
        if out.len() == MAX_MASK_ELEMENTS {
            return Err(A::Error::custom(format!(
                "mask holds more than the {MAX_MASK_ELEMENTS} entries a mask may hold"
            )));
        }
        charge_document_budget::<A::Error>(1)?;
        out.push(value);
    }
    Ok(out)
}

/// `#[serde(with = "...")]` for a required alpha buffer.
pub mod alpha {
    use super::*;

    pub fn serialize<S: Serializer>(alpha: &Vec<f32>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode_alpha(alpha).ok_or_else(encoding_failed)?)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f32>, D::Error> {
        d.deserialize_any(AlphaVisitor)
    }
}

/// `#[serde(with = "...")]` for an optional alpha buffer.
pub mod alpha_opt {
    use super::*;

    pub fn serialize<S: Serializer>(alpha: &Option<Vec<f32>>, s: S) -> Result<S::Ok, S::Error> {
        match alpha {
            Some(alpha) => s.serialize_some(&encode_alpha(alpha).ok_or_else(encoding_failed)?),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<f32>>, D::Error> {
        struct OptVisitor;

        impl<'de> Visitor<'de> for OptVisitor {
            type Value = Option<Vec<f32>>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("null, a packed mask string, or an array of numbers")
            }

            fn visit_none<E: DeError>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E: DeError>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
                d.deserialize_any(AlphaVisitor).map(Some)
            }
        }

        d.deserialize_option(OptVisitor)
    }
}

// ── region labels (`Vec<u32>`) ────────────────────────────────────────────

/// `#[serde(with = "...")]` for a region label buffer. Labels are identities, so
/// this encoding is lossless.
pub mod labels {
    use super::*;

    pub fn serialize<S: Serializer>(labels: &Vec<u32>, s: S) -> Result<S::Ok, S::Error> {
        let mut bytes = Vec::with_capacity(labels.len() * 4);
        for label in labels {
            bytes.extend_from_slice(&label.to_le_bytes());
        }
        let packed = encode(TAG_LABEL_U32, labels.len(), &bytes).ok_or_else(encoding_failed)?;
        s.serialize_str(&packed)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u32>, D::Error> {
        struct LabelVisitor;

        impl<'de> Visitor<'de> for LabelVisitor {
            type Value = Vec<u32>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a packed label string or an array of integers")
            }

            fn visit_str<E: DeError>(self, text: &str) -> Result<Self::Value, E> {
                let bytes = decode::<E>(TAG_LABEL_U32, 4, text)?;
                Ok(bytes
                    .chunks_exact(4)
                    .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect())
            }

            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                read_legacy_seq(seq)
            }
        }

        d.deserialize_any(LabelVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        LocalAdjustmentLayer, LocalEffect, LocalMask, RasterVectorMask, RegionMask, SubjectMask,
        SubjectMaskRefinement,
    };

    #[test]
    fn a_soft_edge_survives_the_round_trip_within_one_step_of_255() {
        let mut mask = RasterVectorMask::empty(8, 1);
        // A feathered brush edge: the values a 1-bit encoding would destroy.
        mask.alpha = vec![0.0, 0.1, 0.25, 0.4, 0.6, 0.75, 0.9, 1.0];
        let json = serde_json::to_string(&mask).unwrap();
        let back: RasterVectorMask = serde_json::from_str(&json).unwrap();
        for (before, after) in mask.alpha.iter().zip(&back.alpha) {
            assert!(
                (before - after).abs() <= 1.0 / 255.0,
                "{before} -> {after} lost more than one quantization step"
            );
        }
        // The extremes have to be exact, or a fully masked pixel would leak.
        assert_eq!(back.alpha.first(), Some(&0.0));
        assert_eq!(back.alpha.last(), Some(&1.0));
    }

    #[test]
    fn the_number_array_written_by_released_versions_still_loads() {
        let before = legacy_decode_count();
        let legacy = r#"{"width":2,"height":2,"alpha":[0.0,1.0,0.5,0.0],"shapes":[]}"#;
        let mask: RasterVectorMask = serde_json::from_str(legacy).unwrap();
        assert_eq!(mask.alpha.len(), 4);
        assert_eq!(mask.alpha[1], 1.0);
        assert!((mask.alpha[2] - 0.5).abs() < 1e-6);
        assert!(
            legacy_decode_count() > before,
            "a legacy read must be visible to the caller that wants to rewrite the file"
        );
    }

    #[test]
    fn a_legacy_read_is_written_back_in_the_packed_form() {
        let legacy = r#"{"width":2,"height":2,"alpha":[0.0,1.0,0.5,0.0],"shapes":[]}"#;
        let mask: RasterVectorMask = serde_json::from_str(legacy).unwrap();
        let json = serde_json::to_string(&mask).unwrap();
        assert!(
            json.contains("\"alpha\":\"q8z:"),
            "re-serialization must not keep the number array: {json}"
        );
    }

    #[test]
    fn the_packed_form_is_orders_of_magnitude_smaller_than_the_array() {
        // 1000x1000 with a small painted blob, the shape real mask data takes.
        let mut mask = RasterVectorMask::empty(1000, 1000);
        for y in 400..430 {
            for x in 400..460 {
                mask.alpha[y * 1000 + x] = 1.0;
            }
        }
        let packed = serde_json::to_string(&mask).unwrap();
        let as_array = mask.alpha.len() * 4; // the shortest a JSON array could be ("0.0,")
        assert!(
            packed.len() * 100 < as_array,
            "packed {} bytes vs at-best array {} bytes",
            packed.len(),
            as_array
        );
    }

    #[test]
    fn an_optional_subject_alpha_keeps_none_none_and_some_some() {
        let mut subject = SubjectMask {
            width: 2,
            height: 2,
            alpha: vec![0.0, 0.25, 0.75, 1.0],
            source_alpha: None,
            refinement: Default::default(),
        };
        let json = serde_json::to_string(&subject).unwrap();
        assert!(!json.contains("source_alpha"), "None must stay skipped");

        subject.source_alpha = Some(vec![1.0, 0.0, 1.0, 0.0]);
        let json = serde_json::to_string(&subject).unwrap();
        let back: SubjectMask = serde_json::from_str(&json).unwrap();
        assert_eq!(back.source_alpha, Some(vec![1.0, 0.0, 1.0, 0.0]));

        // ...and the released array form of the same field.
        let legacy = r#"{"width":2,"height":2,"alpha":[0,0,0,0],"source_alpha":[1.0,0.0,1.0,0.0]}"#;
        let back: SubjectMask = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.source_alpha, Some(vec![1.0, 0.0, 1.0, 0.0]));
    }

    #[test]
    fn region_labels_are_identities_so_they_round_trip_exactly() {
        let mut region = RegionMask::empty(4, 2);
        region.labels = vec![0, 1, 2, 300, 65_536, 4_294_967_295, 7, 0];
        region.selected = vec![false, true, false];
        let json = serde_json::to_string(&region).unwrap();
        let back: RegionMask = serde_json::from_str(&json).unwrap();
        assert_eq!(back.labels, region.labels);
        assert_eq!(back.selected, region.selected);

        let legacy = r#"{"width":2,"height":1,"labels":[0,4294967295],"selected":[false,true]}"#;
        let back: RegionMask = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.labels, vec![0, 4_294_967_295]);
    }

    #[test]
    fn a_truncated_payload_is_refused_rather_than_decoded_short() {
        let mut mask = RasterVectorMask::empty(64, 64);
        mask.alpha[100] = 1.0;
        let json = serde_json::to_string(&mask).unwrap();
        // Drop four base64 characters off the end of the payload, the shape a
        // half-written file takes.
        let close = json
            .rfind("\",\"shapes")
            .expect("alpha is followed by shapes");
        let truncated = format!("{}{}", &json[..close - 4], &json[close..]);
        let error = serde_json::from_str::<RasterVectorMask>(&truncated)
            .expect_err("a short buffer must not pass for a full mask");
        assert!(
            error.to_string().contains("bytes"),
            "the error should say the length disagreed: {error}"
        );
    }

    /// A per-mask ceiling does not bound a file.
    ///
    /// One layer carries a subject mask, its source alpha, and both manual-override
    /// rasters; a sidecar carries any number of layers over any number of items. Each can
    /// sit under `MAX_MASK_ELEMENTS` while the document asks for tens of gigabytes, and
    /// the file that does it is small because a flat alpha plane compresses to nothing.
    #[test]
    fn one_document_cannot_spend_more_than_its_budget_across_many_masks() {
        // A layer whose four mask buffers are individually unremarkable.
        let mut subject = SubjectMask {
            width: 64,
            height: 64,
            alpha: vec![0.0; 4_096],
            source_alpha: Some(vec![0.0; 4_096]),
            refinement: SubjectMaskRefinement::default(),
        };
        subject.alpha[7] = 1.0;
        let mut add = RasterVectorMask::empty(64, 64);
        add.alpha[9] = 1.0;
        let mut layer =
            LocalAdjustmentLayer::new("budgeted", LocalMask::Subject(subject), LocalEffect::None);
        layer.manual_override.add = Some(add.clone());
        layer.manual_override.subtract = Some(add);
        let json = serde_json::to_string(&vec![&layer, &layer, &layer]).unwrap();

        // 3 layers x 4 buffers x 4096 entries x 4 bytes.
        let total_bytes = 3 * 4 * 4_096 * 4;

        {
            let _budget = DocumentBudget::with_bytes(total_bytes);
            let parsed: Vec<LocalAdjustmentLayer> =
                serde_json::from_str(&json).expect("a document that fits its budget must load");
            assert_eq!(parsed.len(), 3);
            assert_eq!(DocumentBudget::remaining(), Some(0));
        }

        {
            // One element short: the file is refused, and no single mask is over the
            // per-mask ceiling, so only the cumulative rule can catch it.
            let _budget = DocumentBudget::with_bytes(total_bytes - 4);
            let error = serde_json::from_str::<Vec<LocalAdjustmentLayer>>(&json)
                .expect_err("a document past its budget must be refused");
            assert!(
                error.to_string().contains("more than it may hold"),
                "the error should name the document budget: {error}"
            );
        }

        // The scope closes with the value it was opened over.
        assert_eq!(DocumentBudget::remaining(), None);
    }

    /// The released number-array form decodes into the same buffers, so it is charged
    /// the same way; a budget that counted only one encoding would be a hole.
    #[test]
    fn the_legacy_number_array_is_charged_against_the_same_budget() {
        let legacy = r#"{"width":2,"height":2,"alpha":[0.0,1.0,0.5,0.0],"shapes":[]}"#;
        {
            let _budget = DocumentBudget::with_bytes(16);
            let mask: RasterVectorMask = serde_json::from_str(legacy).unwrap();
            assert_eq!(mask.alpha.len(), 4);
            assert_eq!(DocumentBudget::remaining(), Some(0));
        }
        {
            let _budget = DocumentBudget::with_bytes(12);
            assert!(serde_json::from_str::<RasterVectorMask>(legacy).is_err());
        }
    }

    /// The count in the header decides an allocation before any of the data behind
    /// it is read, and edit bundles are made to be handed to other people. A payload
    /// of a few bytes must not be able to ask for gigabytes.
    #[test]
    fn a_small_payload_cannot_claim_a_count_past_the_ceiling() {
        let mut mask = RasterVectorMask::empty(8, 8);
        mask.alpha[3] = 1.0;
        let json = serde_json::to_string(&mask).unwrap();
        // The payload stays as it is; only the declared count moves.
        let swapped = json.replace("\"q8z:64:", &format!("\"q8z:{}:", u64::MAX));
        assert_ne!(swapped, json, "the count should have been replaced");
        let error = serde_json::from_str::<RasterVectorMask>(&swapped)
            .expect_err("a count past the ceiling must be refused, not allocated");
        assert!(
            error.to_string().contains("past the"),
            "the error should name the ceiling: {error}"
        );

        // One entry over is refused; the ceiling itself is only rejected later, by the
        // payload not carrying that many bytes -- never by allocating them up front.
        let over = json.replace("\"q8z:64:", &format!("\"q8z:{}:", MAX_MASK_ELEMENTS + 1));
        assert!(serde_json::from_str::<RasterVectorMask>(&over).is_err());
        let at = json.replace("\"q8z:64:", &format!("\"q8z:{MAX_MASK_ELEMENTS}:"));
        let error = serde_json::from_str::<RasterVectorMask>(&at)
            .expect_err("the payload holds 64 entries, not the maximum");
        assert!(
            error.to_string().contains("bytes"),
            "at the ceiling the length check should be what refuses it: {error}"
        );
    }

    #[test]
    fn a_corrupt_payload_fails_instead_of_decoding_to_something_plausible() {
        let cases = [
            r#"{"width":1,"height":1,"alpha":"not base64 at all","shapes":[]}"#,
            r#"{"width":1,"height":1,"alpha":"q8z:1:!!!!","shapes":[]}"#,
            r#"{"width":1,"height":1,"alpha":"q8z:1:AAAA","shapes":[]}"#,
            r#"{"width":1,"height":1,"alpha":"q8z:AAAA","shapes":[]}"#,
            r#"{"width":1,"height":1,"alpha":"u32z:1:AAAA","shapes":[]}"#,
        ];
        for case in cases {
            assert!(
                serde_json::from_str::<RasterVectorMask>(case).is_err(),
                "should have been rejected: {case}"
            );
        }
    }
}
