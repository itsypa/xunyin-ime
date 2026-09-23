//! WINDPKG/WINDEND 格式序列化/反序列化测试
//!
//! 独立集成测试文件（编译为 format_serde-*.exe），
//! 避免 wind_installer-*.exe 因含 "installer" 触发 Windows UAC 启发式检测。

use wind_installer::archive::{
    ArchiveEntry, ArchiveFooter, ArchiveHeader, CompressionType, FORMAT_VERSION,
};

fn make_entry(path: &str, offset: u64, compressed: u64, original: u64, crc: u32) -> ArchiveEntry {
    ArchiveEntry {
        path: path.to_string(),
        offset,
        compressed_size: compressed,
        original_size: original,
        crc32: crc,
    }
}

// ── Header 序列化 ─────────────────────────────────────────────────────────────

#[test]
fn header_roundtrip_preserves_all_fields() {
    let mut h = ArchiveHeader::new(CompressionType::Zstd);
    h.entries
        .push(make_entry("foo/bar.txt", 0, 100, 200, 0xDEAD_BEEF));
    h.entries
        .push(make_entry("baz.bin", 100, 50, 1000, 0x1234_5678));
    h.entry_count = 2;

    let parsed = ArchiveHeader::from_bytes(&h.to_bytes()).unwrap();

    assert_eq!(parsed.version, FORMAT_VERSION);
    assert_eq!(parsed.compression, CompressionType::Zstd);
    assert_eq!(parsed.entry_count, 2);
    assert_eq!(parsed.entries.len(), 2);

    let e0 = &parsed.entries[0];
    assert_eq!(e0.path, "foo/bar.txt");
    assert_eq!(e0.offset, 0);
    assert_eq!(e0.compressed_size, 100);
    assert_eq!(e0.original_size, 200);
    assert_eq!(e0.crc32, 0xDEAD_BEEF);

    let e1 = &parsed.entries[1];
    assert_eq!(e1.path, "baz.bin");
    assert_eq!(e1.compressed_size, 50);
    assert_eq!(e1.original_size, 1000);
    assert_eq!(e1.crc32, 0x1234_5678);
}

#[test]
fn header_roundtrip_lzma_compression_preserved() {
    let h = ArchiveHeader::new(CompressionType::Lzma);
    let parsed = ArchiveHeader::from_bytes(&h.to_bytes()).unwrap();
    assert_eq!(parsed.compression, CompressionType::Lzma);
}

#[test]
fn header_roundtrip_empty_entry_list() {
    let h = ArchiveHeader::new(CompressionType::Zstd);
    let parsed = ArchiveHeader::from_bytes(&h.to_bytes()).unwrap();
    assert_eq!(parsed.entry_count, 0);
    assert!(parsed.entries.is_empty());
}

#[test]
fn header_roundtrip_unicode_path() {
    let mut h = ArchiveHeader::new(CompressionType::Zstd);
    h.entries
        .push(make_entry("数据/字体/黑体字根.ttf", 0, 10, 20, 0));
    h.entry_count = 1;
    let parsed = ArchiveHeader::from_bytes(&h.to_bytes()).unwrap();
    assert_eq!(parsed.entries[0].path, "数据/字体/黑体字根.ttf");
}

#[test]
fn header_roundtrip_preserves_manifest_and_logo() {
    let mut h = ArchiveHeader::new(CompressionType::Lzma);
    h.manifest = b"[app]\nid = \"MyApp\"\n".to_vec();
    h.logo = vec![0x89, 0x50, 0x4E, 0x47, 0x00, 0xFF, 0x42]; // 伪 PNG 头 + 任意字节
    h.entries.push(make_entry("a.txt", 0, 1, 2, 3));
    h.entry_count = 1;

    let parsed = ArchiveHeader::from_bytes(&h.to_bytes()).unwrap();

    assert_eq!(parsed.manifest, h.manifest, "manifest 字节应 roundtrip");
    assert_eq!(parsed.logo, h.logo, "logo 字节应 roundtrip");
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].path, "a.txt");
}

#[test]
fn header_roundtrip_empty_manifest_and_logo() {
    let h = ArchiveHeader::new(CompressionType::Zstd);
    let parsed = ArchiveHeader::from_bytes(&h.to_bytes()).unwrap();
    assert!(parsed.manifest.is_empty());
    assert!(parsed.logo.is_empty());
}

#[test]
fn header_rejects_invalid_magic() {
    let mut bytes = ArchiveHeader::new(CompressionType::Zstd).to_bytes();
    bytes[0] = 0xFF;
    assert!(ArchiveHeader::from_bytes(&bytes).is_err());
}

#[test]
fn header_rejects_unsupported_version() {
    let mut bytes = ArchiveHeader::new(CompressionType::Zstd).to_bytes();
    bytes[8..12].copy_from_slice(&99u32.to_le_bytes()); // version 字段在 bytes[8..12]
    assert!(ArchiveHeader::from_bytes(&bytes).is_err());
}

// ── Footer 序列化 ─────────────────────────────────────────────────────────────

#[test]
fn footer_roundtrip_preserves_header_offset() {
    let offset = 0x1234_5678_9ABC_DEF0u64;
    let bytes = ArchiveFooter::new(offset).to_bytes();
    let parsed = ArchiveFooter::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.header_offset, offset);
}

#[test]
fn footer_roundtrip_zero_offset() {
    let bytes = ArchiveFooter::new(0).to_bytes();
    let parsed = ArchiveFooter::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.header_offset, 0);
}

#[test]
fn footer_roundtrip_max_offset() {
    let bytes = ArchiveFooter::new(u64::MAX).to_bytes();
    let parsed = ArchiveFooter::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.header_offset, u64::MAX);
}

#[test]
fn footer_rejects_invalid_magic() {
    let mut bytes = ArchiveFooter::new(42).to_bytes();
    bytes[8] = 0xFF; // 破坏 footer 魔数（bytes[8..16] 是魔数）
    assert!(ArchiveFooter::from_bytes(&bytes).is_err());
}
