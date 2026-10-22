//! Byte helpers shared by the content decoders and the implicit tiling: the padding of the JSON chunks.

/// A JSON chunk without the padding some writers leave (spaces, zeros)
pub(crate) fn trim_padding(text: &[u8]) -> &[u8] {
    let end = text
        .iter()
        .rposition(|&b| b != 0 && !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);

    &text[..end]
}
