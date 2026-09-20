#![forbid(unsafe_code)]

//! Scalar boundary and safe, scoped object storage.
//! Direct-rustc scalar probes omit the default Cargo `objects` feature.

#[cfg(feature = "objects")]
pub mod grammar;
#[cfg(feature = "objects")]
pub mod native_objects;
#[cfg(feature = "objects")]
pub mod objects;
#[cfg(feature = "objects")]
mod output;
#[cfg(feature = "objects")]
mod regex;
#[cfg(feature = "objects")]
pub mod relations;
#[cfg(feature = "objects")]
pub mod save;
#[cfg(feature = "objects")]
pub mod session;
#[cfg(feature = "objects")]
mod sort;
#[cfg(feature = "objects")]
pub mod tables;

/// Canonical native scalar tags. Every input bit pattern is handled explicitly.
pub extern "C" fn classify(value: u64) -> u32 {
    match value {
        0 => 0,
        1 => 1,
        _ if value as u32 == 2 => 2,
        _ => 255,
    }
}

/// C layout is explicit. This is not a Rust ABI, game address or borrowed buffer.
#[repr(C)]
pub struct ScalarApi {
    pub version: u32,
    pub size: u32,
    pub classify: extern "C" fn(u64) -> u32,
}

/// Keep the normal Rust mangled name; consumers discover it from this exact build.
#[used]
pub static SCALAR_API_V1: ScalarApi = ScalarApi {
    version: 1,
    size: std::mem::size_of::<ScalarApi>() as u32,
    classify,
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_values_and_malformed_bits_are_distinct() {
        for (value, expected) in [
            (0, 0),
            (1, 1),
            (2, 2),
            (0xffffffff00000002, 2),
            (0x8000000000000002, 2),
            (3, 255),
            (u64::MAX, 255),
            (0x100000000, 255),
            (0x100000001, 255),
        ] {
            assert_eq!(classify(value), expected);
            assert_eq!((SCALAR_API_V1.classify)(value), expected);
        }
        assert_eq!(SCALAR_API_V1.version, 1);
        assert_eq!(
            SCALAR_API_V1.size as usize,
            std::mem::size_of::<ScalarApi>()
        );
    }
}
