//! Capture admission only: no image decoding or filesystem access.
//!
//! `CAPTURE_IMAGE_FORMATS` is the public source for the browser extension's
//! extension-set conformance test as well as the Rust capture consumers.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureImageKind {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
    Tiff,
    Avif,
    Heic,
    JpegXl,
}

impl CaptureImageKind {
    pub fn format(self) -> &'static CaptureImageFormat {
        CAPTURE_IMAGE_FORMATS
            .iter()
            .find(|format| format.kind == self)
            .expect("each capture image kind has a format table entry")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureImageSignature {
    Prefix(&'static [u8]),
    /// Unspecified mask bytes are 0xff (exact match).
    MaskedPrefix {
        bytes: &'static [u8],
        mask: &'static [u8],
    },
    /// HEIC retains its existing major-brand admission; AVIF also recognizes
    /// compatible brands. The container layout is ISO BMFF, independent of MIME.
    FtypBrands {
        brands: &'static [&'static [u8; 4]],
        compatible: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureImageFormat {
    pub kind: CaptureImageKind,
    /// First is the save extension. Remaining entries are accepted link aliases.
    pub extensions: &'static [&'static str],
    pub signatures: &'static [CaptureImageSignature],
}

impl CaptureImageFormat {
    pub fn extension(&self) -> &'static str {
        self.extensions[0]
    }
}

/// Matches the released clipboard image-link set. Keep aliases and signatures
/// together; consumers must not maintain their own list of accepted formats.
pub const CAPTURE_IMAGE_FORMATS: &[CaptureImageFormat] = &[
    CaptureImageFormat {
        kind: CaptureImageKind::Png,
        extensions: &["png"],
        signatures: &[CaptureImageSignature::Prefix(b"\x89PNG\r\n\x1a\n")],
    },
    CaptureImageFormat {
        kind: CaptureImageKind::Jpeg,
        extensions: &["jpg", "jpeg"],
        signatures: &[CaptureImageSignature::Prefix(b"\xff\xd8\xff")],
    },
    CaptureImageFormat {
        kind: CaptureImageKind::Gif,
        extensions: &["gif"],
        signatures: &[
            CaptureImageSignature::Prefix(b"GIF87a"),
            CaptureImageSignature::Prefix(b"GIF89a"),
        ],
    },
    CaptureImageFormat {
        kind: CaptureImageKind::WebP,
        extensions: &["webp"],
        signatures: &[CaptureImageSignature::MaskedPrefix {
            bytes: b"RIFF\0\0\0\0WEBP",
            mask: b"\xff\xff\xff\xff\0\0\0\0",
        }],
    },
    CaptureImageFormat {
        kind: CaptureImageKind::Bmp,
        extensions: &["bmp"],
        signatures: &[CaptureImageSignature::Prefix(b"BM")],
    },
    CaptureImageFormat {
        kind: CaptureImageKind::Tiff,
        extensions: &["tiff", "tif"],
        signatures: &[
            CaptureImageSignature::Prefix(b"II*\0"),
            CaptureImageSignature::Prefix(b"MM\0*"),
        ],
    },
    // AVIF precedes HEIC so a compatible AVIF brand wins over a HEIF major brand.
    CaptureImageFormat {
        kind: CaptureImageKind::Avif,
        extensions: &["avif"],
        signatures: &[CaptureImageSignature::FtypBrands {
            brands: &[b"avif", b"avis"],
            compatible: true,
        }],
    },
    CaptureImageFormat {
        kind: CaptureImageKind::Heic,
        extensions: &["heic", "heif"],
        signatures: &[CaptureImageSignature::FtypBrands {
            brands: &[b"heic", b"heix", b"hevc", b"hevx", b"mif1", b"msf1"],
            compatible: false,
        }],
    },
    CaptureImageFormat {
        kind: CaptureImageKind::JpegXl,
        extensions: &["jxl"],
        signatures: &[
            CaptureImageSignature::Prefix(b"\xff\x0a"),
            CaptureImageSignature::Prefix(b"\0\0\0\x0cJXL \r\n\x87\n"),
        ],
    },
];

pub fn capture_image_format_from_extension(extension: &str) -> Option<&'static CaptureImageFormat> {
    CAPTURE_IMAGE_FORMATS.iter().find(|format| {
        format
            .extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    })
}

fn prefix_matches(signature: CaptureImageSignature, bytes: &[u8]) -> bool {
    match signature {
        CaptureImageSignature::Prefix(prefix) => bytes.starts_with(prefix),
        CaptureImageSignature::MaskedPrefix {
            bytes: prefix,
            mask,
        } => {
            bytes.len() >= prefix.len()
                && prefix.iter().enumerate().all(|(index, expected)| {
                    bytes[index] & mask.get(index).copied().unwrap_or(0xff) == *expected
                })
        }
        CaptureImageSignature::FtypBrands { brands, .. } => {
            bytes.len() >= 12
                && &bytes[4..8] == b"ftyp"
                && brands.iter().any(|brand| &bytes[8..12] == brand.as_slice())
        }
    }
}

/// Probe a possibly partial file header for decoder routing. Retains clipboard's
/// existing major-brand detection even when the full ftyp box is not buffered.
/// This does not validate a complete BMFF box; the decoder still validates data.
pub fn capture_image_format_from_header(bytes: &[u8]) -> Option<&'static CaptureImageFormat> {
    CAPTURE_IMAGE_FORMATS.iter().find(|format| {
        format
            .signatures
            .iter()
            .any(|signature| prefix_matches(*signature, bytes))
    })
}

fn ftyp_brands(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return None;
    }
    let length = u32::from_be_bytes(bytes[..4].try_into().ok()?);
    let (length, header) = match length {
        0 => (bytes.len(), 8),
        1 => (
            usize::try_from(u64::from_be_bytes(bytes[8..16].try_into().ok()?)).ok()?,
            16,
        ),
        length => (length as usize, 8),
    };
    if length < header + 8 || length > bytes.len() || !(length - header - 8).is_multiple_of(4) {
        return None;
    }
    Some((&bytes[header..header + 4], &bytes[header + 8..length]))
}

/// Determine the save format from original bytes, never MIME or a filename.
/// BMFF must contain a complete, structurally valid ftyp box. Signature admission
/// is not a full image decode (the host deliberately does not decode images).
pub fn capture_image_format_from_bytes(bytes: &[u8]) -> Option<&'static CaptureImageFormat> {
    CAPTURE_IMAGE_FORMATS.iter().find(|format| {
        format.signatures.iter().any(|signature| match signature {
            CaptureImageSignature::FtypBrands { brands, compatible } => ftyp_brands(bytes)
                .is_some_and(|(major, others)| {
                    brands.iter().any(|brand| major == brand.as_slice())
                        || (*compatible
                            && others.chunks_exact(4).any(|candidate| {
                                brands.iter().any(|brand| candidate == brand.as_slice())
                            }))
                }),
            signature => prefix_matches(*signature, bytes),
        })
    })
}
