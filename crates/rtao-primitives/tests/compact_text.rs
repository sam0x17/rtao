#![allow(clippy::unwrap_used)]
//! Compact bytes must use their metadata wire shape, including non-ASCII values.
use parity_scale_codec::{Compact, Decode, Encode};
use rtao_primitives::CompactBytesText;
use scale_decode::DecodeAsType;
use scale_info::{PortableRegistry, Registry, meta_type};

#[test]
fn compact_text_preserves_threshold_and_non_utf8_bytes() {
    let text = CompactBytesText(vec![0, 63, 64, 255]);
    let encoded = text.encode();
    assert_eq!(encoded, vec![16, 0, 252, 1, 1, 253, 3]);
    assert_ne!(encoded, text.0.encode());
    assert_eq!(CompactBytesText::decode(&mut &encoded[..]).unwrap(), text);
    let mut registry = Registry::new();
    let id = registry.register_type(&meta_type::<Vec<Compact<u8>>>());
    let registry: PortableRegistry = registry.into();
    assert_eq!(
        CompactBytesText::decode_as_type(&mut &encoded[..], id.id, &registry).unwrap(),
        text
    );
    assert!(text.as_str().is_err());
    assert_eq!(CompactBytesText::from("τ").as_str().unwrap(), "τ");
}
