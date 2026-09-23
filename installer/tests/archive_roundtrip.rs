//! 归档读写集成测试
//!
//! 关键回归测试：`*_full_content_not_truncated` 系列
//! 复现修复前的 Bug：`Read::take(decoder, compressed_size)` 用压缩大小截断解压输出，
//! 导致任何能被压缩的文件解压后都是残缺的，CRC 校验必然失败。

use std::path::{Path, PathBuf};
use wind_installer::archive::{self, ArchiveReader, ArchiveWriter, CompressionType};

// ── 测试辅助 ─────────────────────────────────────────────────────────────────

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(suffix: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("wind_test_{}_{}", std::process::id(), suffix));
        std::fs::create_dir_all(&path).expect("创建临时目录失败");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write_file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("创建父目录失败");
        }
        std::fs::write(&path, content).expect("写入测试文件失败");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn pack(
    dir: &TempDir,
    archive_name: &str,
    files: &[(&str, &[u8])],
    compression: CompressionType,
) -> PathBuf {
    let archive_path = dir.path().join(archive_name);
    let mut writer =
        ArchiveWriter::new(&archive_path, compression).expect("创建 ArchiveWriter 失败");
    for (name, content) in files {
        let src = dir.write_file(name, content);
        writer.add_file(&src, name).expect("add_file 失败");
    }
    writer.finish().expect("finish 失败");
    archive_path
}

fn extract_all_to(archive_path: &Path, out_dir: &Path) {
    std::fs::create_dir_all(out_dir).unwrap();
    let mut reader = ArchiveReader::open(archive_path).expect("打开归档失败");
    let entries = reader.entries().to_vec();
    for entry in &entries {
        let dest = out_dir.join(&entry.path);
        reader
            .extract_entry(entry, &dest)
            .expect("extract_entry 失败");
    }
}

// ── 核心回归测试：捕获 Read::take 截断 Bug ───────────────────────────────────

/// Zstd 关键回归测试。
///
/// 10KB 重复字节 → zstd 压缩后约 20-30 字节（compressed_size ≈ 25）。
/// 旧 Bug：`take(decoder, 25)` 只允许解压器输出 25 字节，文件被截断。
/// 正确行为：`take` 限制压缩输入消耗，解压器输出完整 10240 字节。
#[test]
fn zstd_decompresses_full_content_when_compression_ratio_is_high() {
    let dir = TempDir::new("zstd_full");
    let original = vec![0xABu8; 10 * 1024];

    let archive = pack(
        &dir,
        "test.pkg",
        &[("data.bin", &original)],
        CompressionType::Zstd,
    );
    let out_dir = dir.path().join("out");
    extract_all_to(&archive, &out_dir);

    let extracted = std::fs::read(out_dir.join("data.bin")).expect("读取解压文件失败");
    assert_eq!(
        extracted.len(),
        original.len(),
        "Zstd 解压后大小 {} 与原始 {} 不符（可能被截断为 compressed_size）",
        extracted.len(),
        original.len()
    );
    assert_eq!(extracted, original, "Zstd 解压内容应与原始完全一致");
}

/// LZMA 关键回归测试（与 Zstd 相同场景）。
#[test]
fn lzma_decompresses_full_content_when_compression_ratio_is_high() {
    let dir = TempDir::new("lzma_full");
    let original = vec![0xCDu8; 10 * 1024];

    let archive = pack(
        &dir,
        "test.pkg",
        &[("data.bin", &original)],
        CompressionType::Lzma,
    );
    let out_dir = dir.path().join("out");
    extract_all_to(&archive, &out_dir);

    let extracted = std::fs::read(out_dir.join("data.bin")).expect("读取解压文件失败");
    assert_eq!(
        extracted.len(),
        original.len(),
        "LZMA 解压后大小 {} 与原始 {} 不符",
        extracted.len(),
        original.len()
    );
    assert_eq!(extracted, original, "LZMA 解压内容应与原始完全一致");
}

// ── 多文件测试 ────────────────────────────────────────────────────────────────

/// 多个文件全部正确解压，内容互不干扰。
#[test]
fn multiple_entries_all_extracted_correctly() {
    let dir = TempDir::new("multi");
    let files: &[(&str, &[u8])] = &[
        ("a.txt", b"hello world"),
        ("sub/b.bin", &[0u8; 4096]), // 4KB 零字节，高压缩率
        ("c.dat", b"the quick brown fox jumps over the lazy dog"),
    ];

    let archive = pack(&dir, "multi.pkg", files, CompressionType::Zstd);
    let out_dir = dir.path().join("out");
    extract_all_to(&archive, &out_dir);

    for (name, content) in files {
        let extracted =
            std::fs::read(out_dir.join(name)).unwrap_or_else(|_| panic!("读取 {} 失败", name));
        assert_eq!(&extracted, content, "文件 {} 内容不一致", name);
    }
}

/// 验证顺序提取时文件指针在每个条目后正确推进。
/// 三个大小各异的文件，任何一个错位都会导致内容错乱。
#[test]
fn sequential_entries_file_pointer_advances_correctly() {
    let dir = TempDir::new("seq");
    let file_a = vec![0x01u8; 5000];
    let file_b = vec![0x02u8; 2000];
    let file_c = vec![0x03u8; 8000];

    let archive = pack(
        &dir,
        "seq.pkg",
        &[("a.bin", &file_a), ("b.bin", &file_b), ("c.bin", &file_c)],
        CompressionType::Zstd,
    );
    let out_dir = dir.path().join("out");
    extract_all_to(&archive, &out_dir);

    assert_eq!(
        std::fs::read(out_dir.join("a.bin")).unwrap(),
        file_a,
        "a.bin 内容错误"
    );
    assert_eq!(
        std::fs::read(out_dir.join("b.bin")).unwrap(),
        file_b,
        "b.bin 内容错误"
    );
    assert_eq!(
        std::fs::read(out_dir.join("c.bin")).unwrap(),
        file_c,
        "c.bin 内容错误"
    );
}

// ── 边界情况 ──────────────────────────────────────────────────────────────────

/// 所有字节值 0x00-0xFF 都被精确保留（验证二进制安全）。
#[test]
fn binary_content_all_byte_values_preserved() {
    let dir = TempDir::new("binary");
    let original: Vec<u8> = (0u8..=255).cycle().take(2048).collect();

    let archive = pack(
        &dir,
        "bin.pkg",
        &[("all_bytes.bin", &original)],
        CompressionType::Zstd,
    );
    let out_dir = dir.path().join("out");
    extract_all_to(&archive, &out_dir);

    let extracted = std::fs::read(out_dir.join("all_bytes.bin")).unwrap();
    assert_eq!(extracted, original, "二进制内容应逐字节一致");
}

/// 空文件打包后可正常解压，结果仍为空。
#[test]
fn empty_file_roundtrip() {
    let dir = TempDir::new("empty");

    let archive = pack(
        &dir,
        "empty.pkg",
        &[("empty.bin", b"")],
        CompressionType::Zstd,
    );
    let out_dir = dir.path().join("out");
    extract_all_to(&archive, &out_dir);

    let extracted = std::fs::read(out_dir.join("empty.bin")).unwrap();
    assert!(extracted.is_empty(), "空文件解压后应为空");
}

// ── CRC 校验 ──────────────────────────────────────────────────────────────────

/// 篡改归档压缩数据后，extract_entry 应返回错误（解压失败或 CRC 不匹配）。
#[test]
fn crc_mismatch_or_decompress_error_when_archive_corrupted() {
    let dir = TempDir::new("crc");
    let original = vec![0xAAu8; 1024];

    let archive_path = pack(
        &dir,
        "crc.pkg",
        &[("data.bin", &original)],
        CompressionType::Zstd,
    );

    // 篡改 offset=0 处的压缩块前 4 字节
    let mut raw = std::fs::read(&archive_path).unwrap();
    raw[0] ^= 0xFF;
    raw[1] ^= 0xFF;
    raw[2] ^= 0xFF;
    raw[3] ^= 0xFF;
    std::fs::write(&archive_path, &raw).unwrap();

    let out_dir = dir.path().join("out");
    std::fs::create_dir_all(&out_dir).unwrap();

    let mut reader = ArchiveReader::open(&archive_path).unwrap();
    let entries = reader.entries().to_vec();
    let result = reader.extract_entry(&entries[0], &out_dir.join("data.bin"));

    assert!(result.is_err(), "篡改归档后应返回错误，实际返回 Ok");
}

// ── Bundle 流程测试 ───────────────────────────────────────────────────────────

/// 核心回归测试：复现 dist/ 下安装包 "Invalid header magic" 的根本原因。
///
/// bundle_exe 直接拼接 stub+archive 时，archive 内所有偏移都是相对 .bin 起点的。
/// 若不修正偏移（各加 stub_size），读取器会寻址到 stub 内部并读出垃圾数据。
#[test]
fn bundle_exe_fixes_offsets_so_content_readable_after_prepending_stub() {
    let dir = TempDir::new("bundle");
    let content_a = vec![0xAAu8; 4096]; // 高压缩率：compressed_size << original_size
    let content_b = b"hello from file B".as_slice();

    // 1. Pack → .bin
    let archive = pack(
        &dir,
        "data.bin",
        &[("a.dat", &content_a), ("b.txt", content_b)],
        CompressionType::Zstd,
    );

    // 2. 创建一个假 stub（模拟 wind-installer.exe 的二进制头部）
    //    使用随机字节确保偏移计算不依赖内容
    let stub_content: Vec<u8> = (0u8..=255).cycle().take(8192).collect();
    let stub_path = dir.write_file("stub.exe", &stub_content);

    // 3. Bundle：stub + 修正偏移后的 archive
    let installer = dir.path().join("installer.exe");
    archive::bundle_exe(&stub_path, &archive, &installer).expect("bundle_exe 应成功");

    // 4. 从合并后的 installer.exe 中读取归档
    let out_dir = dir.path().join("out");
    extract_all_to(&installer, &out_dir);

    // 5. 验证内容完整
    let extracted_a = std::fs::read(out_dir.join("a.dat")).expect("读取 a.dat 失败");
    let extracted_b = std::fs::read(out_dir.join("b.txt")).expect("读取 b.txt 失败");

    assert_eq!(
        extracted_a, content_a,
        "a.dat 内容不一致（高压缩率文件在偏移修正前会被截断）"
    );
    assert_eq!(extracted_b, content_b, "b.txt 内容不一致");
}

/// 核心回归测试：清单 + logo 经 pack → bundle → read 全程保持完整。
///
/// 验证通用安装器的关键链路：打包时嵌入的 manifest/logo 字节，在追加 stub overlay
/// 后仍能被运行期从最终 exe 的头部正确读出（bundle_exe 原样复制头部字节）。
#[test]
fn manifest_and_logo_survive_pack_and_bundle() {
    let dir = TempDir::new("manifest_bundle");
    // display_name 特意用非 ASCII：清单以字节原样往返，UTF-8 不应在任何一环被改写
    let manifest =
        b"[app]\nid = \"DemoApp\"\ndisplay_name = \"\xe7\xa4\xba\xe4\xbe\x8b\"\n".to_vec();
    let logo = vec![0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x11, 0x22]; // 伪 PNG

    // pack：写入归档并设置清单/logo
    let archive_path = dir.path().join("data.bin");
    let mut writer = ArchiveWriter::new(&archive_path, CompressionType::Lzma).unwrap();
    writer.set_manifest(manifest.clone(), logo.clone());
    let src = dir.write_file("payload.bin", &vec![0x5Au8; 3000]);
    writer.add_file(&src, "payload.bin").unwrap();
    writer.finish().unwrap();

    // 直接从 .bin 读回
    let reader = ArchiveReader::open(&archive_path).unwrap();
    assert_eq!(reader.manifest_bytes(), &manifest[..], ".bin 清单应一致");
    assert_eq!(reader.logo_bytes(), &logo[..], ".bin logo 应一致");

    // bundle：追加 stub overlay 后从最终 exe 读回
    let stub: Vec<u8> = (0u8..=255).cycle().take(6000).collect();
    let stub_path = dir.write_file("stub.exe", &stub);
    let installer = dir.path().join("setup.exe");
    archive::bundle_exe(&stub_path, &archive_path, &installer).unwrap();

    let reader2 = ArchiveReader::open(&installer).unwrap();
    assert_eq!(
        reader2.manifest_bytes(),
        &manifest[..],
        "bundle 后清单应一致"
    );
    assert_eq!(reader2.logo_bytes(), &logo[..], "bundle 后 logo 应一致");
    // 数据仍可正确解压
    let out_dir = dir.path().join("out");
    extract_all_to(&installer, &out_dir);
    assert_eq!(
        std::fs::read(out_dir.join("payload.bin")).unwrap(),
        vec![0x5Au8; 3000]
    );
}

/// 卸载器自包含：给裸 exe 追加「仅清单 overlay」后，可从该 exe 自身读回清单/logo，
/// 且无文件条目。验证安装目录无需散落 .manifest/.logo 文件。
#[test]
fn append_manifest_overlay_makes_exe_self_describing() {
    let dir = TempDir::new("overlay");
    // 模拟被解压到安装目录的卸载器 exe（任意 PE 字节）
    let exe_bytes: Vec<u8> = (0u8..=255).cycle().take(7000).collect();
    let exe = dir.write_file("uninstall.exe", &exe_bytes);

    let manifest = b"[app]\nid = \"DemoApp\"\nmain_exe = \"demo_app.exe\"\n".to_vec();
    let logo = vec![0x89u8, 0x50, 0x4E, 0x47, 0xAB, 0xCD];

    archive::append_manifest_overlay(&exe, &manifest, &logo).expect("追加 overlay 失败");

    // 卸载器启动时即从自身读取
    let reader = ArchiveReader::open(&exe).expect("打开追加 overlay 后的 exe 失败");
    assert_eq!(reader.manifest_bytes(), &manifest[..], "清单应可从自身读回");
    assert_eq!(reader.logo_bytes(), &logo[..], "logo 应可从自身读回");
    assert!(reader.entries().is_empty(), "overlay 不含文件条目");

    // 原 exe 字节未被破坏（仍是文件前缀）
    let after = std::fs::read(&exe).unwrap();
    assert_eq!(
        &after[..exe_bytes.len()],
        &exe_bytes[..],
        "原 exe 内容应保持不变"
    );
}

/// bundle_exe 对 stub_size=0（无前缀）应与直接读取 .bin 等价。
#[test]
fn bundle_exe_with_empty_stub_reads_identically_to_plain_archive() {
    let dir = TempDir::new("bundle_empty_stub");
    let content = b"stub size zero test content";

    let archive = pack(
        &dir,
        "data.bin",
        &[("f.txt", content)],
        CompressionType::Zstd,
    );

    // 空 stub
    let stub_path = dir.write_file("empty.exe", b"");
    let bundled = dir.path().join("out.exe");
    archive::bundle_exe(&stub_path, &archive, &bundled).expect("bundle_exe 失败");

    // 从 bundled 读取
    let out_dir = dir.path().join("out");
    extract_all_to(&bundled, &out_dir);
    let extracted = std::fs::read(out_dir.join("f.txt")).unwrap();
    assert_eq!(&extracted, content);
}

// ── Authenticode 共存回归 ────────────────────────────────────────────────────

/// 构造一个「头部合法、其余填充」的最小 PE，用作可被 `sign_in_place` 加签的假 stub。
///
/// 只铺到能让 `archive_end` 走完解析所需的字段：DOS 头的 e_lfanew、PE 签名、
/// Optional Header 的 Magic 与 NumberOfRvaAndSizes、以及第 5 个数据目录项
/// （IMAGE_DIRECTORY_ENTRY_SECURITY）。其余字节是填充，不参与本测试的判据。
fn fake_pe_stub(size: usize) -> Vec<u8> {
    const PE_OFF: usize = 0x80;
    let opt_off = PE_OFF + 24; // PE 签名(4) + COFF 头(20)
    let dd_off = opt_off + 112; // PE32+ 的数据目录起点
    assert!(size > dd_off + 16 * 8, "stub 太小，放不下数据目录");

    let mut b: Vec<u8> = (0u8..=255).cycle().take(size).collect();
    b[0..2].copy_from_slice(b"MZ");
    b[0x3C..0x40].copy_from_slice(&(PE_OFF as u32).to_le_bytes());
    b[PE_OFF..PE_OFF + 4].copy_from_slice(&[b'P', b'E', 0, 0]);
    b[opt_off..opt_off + 2].copy_from_slice(&0x20Bu16.to_le_bytes()); // PE32+
    b[dd_off - 4..dd_off].copy_from_slice(&16u32.to_le_bytes()); // NumberOfRvaAndSizes
                                                                 // Security 目录项（索引 4）先置零 = 未签名
    b[dd_off + 32..dd_off + 40].copy_from_slice(&[0u8; 8]);
    b
}

/// 模拟 signtool：把证书表追加到 PE 物理末尾，并把它的**文件偏移**与长度写进
/// IMAGE_DIRECTORY_ENTRY_SECURITY。返回归档末尾（= 补齐填充之前的文件长度）。
///
/// ⚠️ 必须复现 **8 字节对齐填充**：证书表要求 8 字节对齐，signtool 会先把原文件补齐
/// 到边界再追加。省掉这一步的话，测试只覆盖「原大小恰为 8 的倍数」这七分之一情形，
/// 通过了也说明不了什么 —— 本仓真实的 Setup.exe 就是补了 7 字节，而当时全绿的测试
/// 一个都没拦住（21997457 % 8 == 1）。
fn sign_in_place(path: &Path, cert_size: usize) -> u64 {
    let mut b = std::fs::read(path).expect("读取待签文件失败");
    let archive_end = b.len() as u64;

    // 对齐填充：原文件末尾补 0..=7 字节，让证书表落在 8 字节边界上
    let pad = (8 - (b.len() % 8)) % 8;
    b.extend(std::iter::repeat_n(0u8, pad));
    let cert_off = b.len() as u32;

    let pe_off = u32::from_le_bytes(b[0x3C..0x40].try_into().unwrap()) as usize;
    let dd_off = pe_off + 24 + 112;
    let sec = dd_off + 32;
    b[sec..sec + 4].copy_from_slice(&cert_off.to_le_bytes());
    b[sec + 4..sec + 8].copy_from_slice(&(cert_size as u32).to_le_bytes());

    b.extend(std::iter::repeat_n(0xC7u8, cert_size)); // 假证书表内容，不参与判据
    std::fs::write(path, &b).expect("写回已签文件失败");
    archive_end
}

/// 核心回归测试：**签名后的安装包仍能解包**。
///
/// Authenticode 把证书表追加在 PE 末尾，Footer 因此不再位于文件尾部。修复前
/// `ArchiveReader::open` 从物理末尾往回读 16 字节，读到的是证书表尾巴，
/// magic 校验直接失败 —— 表现为「安装包一签名就报 Invalid footer magic」。
///
/// 实测佐证（自签名证书 + signtool /fd SHA256）：
///   签名前 size=1195024，末 16 字节 = ...57494e44454e4400 ("WINDEND\0")
///   签名后 size=1196440，末 16 字节 = 39dbc8839121f206... (证书表数据)
/// 逐个走完 8 种对齐余数。**必须遍历**：只测一种的话有 7/8 的概率恰好落在
/// 「原大小已是 8 的倍数、无需填充」这条最简单的路径上而全绿，真包照样打不开。
#[test]
fn signed_installer_still_extractable() {
    let content_a = vec![0xAAu8; 4096];
    let content_b = b"hello from file B".as_slice();

    for k in 0..8usize {
        let dir = TempDir::new(&format!("signed{}", k));
        let archive = pack(
            &dir,
            "data.bin",
            &[("a.dat", &content_a), ("b.txt", content_b)],
            CompressionType::Zstd,
        );
        // 逐字节加长 stub，把归档末尾推过 8 种对齐余数
        let stub_path = dir.write_file("stub.exe", &fake_pe_stub(8192 + k));
        let installer = dir.path().join("installer.exe");
        archive::bundle_exe(&stub_path, &archive, &installer).expect("bundle_exe 应成功");

        // 签名前先确认这份包本来是好的，免得测试在「本就打不开」上假绿
        extract_all_to(&installer, &dir.path().join("out_unsigned"));

        let archive_end = sign_in_place(&installer, 1416);
        let signed_len = std::fs::metadata(&installer).unwrap().len();
        assert!(signed_len > archive_end, "签名应当让文件变长 (k={})", k);

        let out_dir = dir.path().join("out_signed");
        extract_all_to(&installer, &out_dir);
        assert_eq!(
            std::fs::read(out_dir.join("a.dat")).expect("读取 a.dat 失败"),
            content_a,
            "签名后 a.dat 内容不一致 (对齐余数 k={}, 填充 {} 字节)",
            k,
            (8 - (archive_end % 8)) % 8
        );
        assert_eq!(
            std::fs::read(out_dir.join("b.txt")).expect("读取 b.txt 失败"),
            content_b,
            "签名后 b.txt 内容不一致 (k={})",
            k
        );
    }
}

/// 清单/logo 走的是同一条 Footer→Header 寻址路径，签名后同样必须读得出来
/// （卸载器 UI 的品牌信息全靠它）。
#[test]
fn signed_installer_still_exposes_manifest_and_logo() {
    let dir = TempDir::new("signed_meta");
    let manifest = b"[app]\nname = \"WindInput\"\n".as_slice();
    let logo = vec![0x89u8; 512];

    let archive_path = dir.path().join("meta.bin");
    let mut writer =
        ArchiveWriter::new(&archive_path, CompressionType::Zstd).expect("创建 writer 失败");
    writer.set_manifest(manifest.to_vec(), logo.clone());
    let src = dir.write_file("f.txt", b"x");
    writer.add_file(&src, "f.txt").expect("add_file 失败");
    writer.finish().expect("finish 失败");

    let stub_path = dir.write_file("stub.exe", &fake_pe_stub(8192));
    let installer = dir.path().join("installer.exe");
    archive::bundle_exe(&stub_path, &archive_path, &installer).unwrap();
    sign_in_place(&installer, 2048);

    let reader = ArchiveReader::open(&installer).expect("签名后应仍能打开归档");
    assert_eq!(reader.header().manifest, manifest, "签名后清单字节不一致");
    assert_eq!(reader.header().logo, logo, "签名后 logo 字节不一致");
}

/// 反向判据：Security 目录项存在、但证书表没有恰好占满文件尾部（offset+size != 文件长度）
/// 时，必须回落到物理末尾，而不是拿这个不自洽的偏移去读 Footer。
///
/// 这一支保护的是「未签名的正常包」：某些 PE 的 Security 项残留非零值，若无条件相信它，
/// 本来好好的安装包反而会被读坏。
#[test]
fn inconsistent_security_directory_falls_back_to_physical_end() {
    let dir = TempDir::new("bad_secdir");
    let content = b"payload stays readable".as_slice();
    let archive = pack(
        &dir,
        "data.bin",
        &[("c.txt", content)],
        CompressionType::Zstd,
    );
    let stub_path = dir.write_file("stub.exe", &fake_pe_stub(8192));
    let installer = dir.path().join("installer.exe");
    archive::bundle_exe(&stub_path, &archive, &installer).unwrap();

    // 写入一个「指向文件中间、且 offset+size 对不上文件长度」的伪证书表，不追加任何字节
    let mut b = std::fs::read(&installer).unwrap();
    let pe_off = u32::from_le_bytes(b[0x3C..0x40].try_into().unwrap()) as usize;
    let sec = pe_off + 24 + 112 + 32;
    b[sec..sec + 4].copy_from_slice(&4096u32.to_le_bytes());
    b[sec + 4..sec + 8].copy_from_slice(&16u32.to_le_bytes());
    std::fs::write(&installer, &b).unwrap();

    let out_dir = dir.path().join("out");
    extract_all_to(&installer, &out_dir);
    assert_eq!(
        std::fs::read(out_dir.join("c.txt")).expect("读取 c.txt 失败"),
        content,
        "不自洽的 Security 目录项应被忽略，归档仍按物理末尾解析"
    );
}

// ── 卸载器清单 overlay 与签名共存 ────────────────────────────────────────────

/// 裸 stub 上 `has_manifest_overlay` 必须为假 —— 否则打包期会跳过加工，
/// 装出来的卸载器读不到清单，启动即失败。
#[test]
fn bare_stub_has_no_manifest_overlay() {
    let dir = TempDir::new("overlay_bare");
    let stub = dir.write_file("uninstall.exe", &fake_pe_stub(8192));
    assert!(
        !archive::has_manifest_overlay(&stub),
        "未加工的裸 stub 不该被判定为已带 overlay"
    );
}

/// 核心回归：**卸载器带 overlay 之后再签名，仍读得回清单，且判据仍为真**。
///
/// 这两条断言各自守着一个「会静默出坏包」的失败模式：
/// - 读不回清单 → 卸载器启动即报「无法载入卸载清单」，而安装包本身验签通过，
///   从外面完全看不出来；
/// - 判据为假 → 安装期的 `AppendUninstallerOverlay` 会再追加一次，把证书表顶到
///   文件中间，Authenticode 判定为"No signature found"，签名白签。
///
/// 遍历 8 种对齐余数：signtool 追加证书表前会把原文件补 0..=7 字节对齐到 8 边界，
/// 只测一种的话有 7/8 概率落在「无需填充」那条最简单的路径上而假绿（本仓真实的
/// Setup.exe 就补了 7 字节，当时全绿的测试一个都没拦住）。
#[test]
fn signed_uninstaller_overlay_stays_readable() {
    let manifest = b"[app]\nid = \"WindInput\"\nversion = \"1.2.3\"\n".as_slice();
    let logo = vec![0x89u8; 777];

    for k in 0..8usize {
        let dir = TempDir::new(&format!("overlay_signed{}", k));
        // 逐字节加长 stub，把 overlay 末尾推过 8 种对齐余数
        let uninst = dir.write_file("uninstall.exe", &fake_pe_stub(8192 + k));

        archive::append_manifest_overlay(&uninst, manifest, &logo).expect("追加 overlay 失败");
        assert!(
            archive::has_manifest_overlay(&uninst),
            "追加后判据应为真 (k={})",
            k
        );

        let archive_end = sign_in_place(&uninst, 1416);
        let signed_len = std::fs::metadata(&uninst).unwrap().len();
        assert!(signed_len > archive_end, "签名应当让文件变长 (k={})", k);

        assert!(
            archive::has_manifest_overlay(&uninst),
            "签名后判据仍须为真, 否则安装期会重复追加并毁掉签名 (k={}, 填充 {} 字节)",
            k,
            (8 - (archive_end % 8)) % 8
        );

        let reader = ArchiveReader::open(&uninst).expect("签名后应仍能打开 overlay");
        assert_eq!(
            reader.header().manifest,
            manifest,
            "清单字节不一致 (k={})",
            k
        );
        assert_eq!(reader.header().logo, logo, "logo 字节不一致 (k={})", k);
        assert_eq!(
            reader.header().entry_count,
            0,
            "overlay 不含文件条目 (k={})",
            k
        );
    }
}

/// overlay 里没有清单时判据为假 —— `has_manifest_overlay` 问的是「清单在不在」，
/// 不是「尾部有没有归档结构」。空清单的 overlay 等于没加工，必须允许补一次。
#[test]
fn empty_manifest_overlay_is_not_treated_as_prepared() {
    let dir = TempDir::new("overlay_empty");
    let uninst = dir.write_file("uninstall.exe", &fake_pe_stub(8192));
    archive::append_manifest_overlay(&uninst, b"", &[]).expect("追加空 overlay 失败");
    assert!(
        !archive::has_manifest_overlay(&uninst),
        "空清单不该被当成已加工"
    );
}

/// `read_manifest_overlay` 必须把 overlay 的**内容**交出来，不能只回答有无。
///
/// 打包器据此判断「这个卸载器是不是本轮配置产的」——只问有无的话，源目录里留着上一轮
/// 的产物就会被当成已就绪，上一版的卸载器原样封进包：包能装、验签也过，只有卸载器按
/// 旧清单走。签名后仍要读得准，因为判据发生在签名之后（prep → 签名 → pack）。
#[test]
fn manifest_overlay_content_is_readable_for_drift_check() {
    let dir = TempDir::new("overlay_drift");
    let v1 = b"[app]\nversion = \"1.0.0\"\n".as_slice();
    let logo = vec![0x42u8; 64];

    let uninst = dir.write_file("uninstall.exe", &fake_pe_stub(8192));
    assert_eq!(
        archive::read_manifest_overlay(&uninst),
        None,
        "裸 stub 读不出 overlay"
    );

    archive::append_manifest_overlay(&uninst, v1, &logo).expect("追加 overlay 失败");
    sign_in_place(&uninst, 1416);

    let (got_manifest, got_logo) =
        archive::read_manifest_overlay(&uninst).expect("签名后仍应读得出 overlay");
    assert_eq!(got_manifest, v1, "清单字节要能原样取回来才谈得上比对");
    assert_eq!(got_logo, logo, "logo 换了同样算配置漂移，故也要能取回");

    // 换一版配置：比对结果必须是「不一致」，让打包器有机会报错而不是静默沿用旧包。
    let v2 = b"[app]\nversion = \"1.0.1\"\n".as_slice();
    assert_ne!(got_manifest, v2, "版本号变了应判为不一致");
}

/// 三条决策路径各自对应一种截然不同的处置，走错任何一条都是静默的坏包：
/// 该加工的没加工 → 装机端读不到清单、启动即失败；该跳过的又加工了一遍 → 毁掉刚打上
/// 的签名；该报错的当成一致 → 上一版卸载器原样封进包，而验签还照样通过。
///
/// 判据住在 lib 而不是 `wind-packer` 里，是因为那个 bin 有 `required-features = ["packer"]`，
/// 而 CI 跑的是 `cargo test --locked --tests` —— 不带 feature 就不构建它，测试连同它要守的
/// 判据一起静默失效。
#[test]
fn overlay_classification_covers_three_decisions() {
    use wind_installer::archive::OverlayState;

    let manifest = b"[app]\nversion = \"1.0.0\"\n".as_slice();
    let logo = b"PNG-A".as_slice();

    assert_eq!(
        archive::classify_overlay(None, manifest, logo),
        OverlayState::Absent,
        "裸 stub 该加工"
    );
    assert_eq!(
        archive::classify_overlay(Some((manifest.to_vec(), logo.to_vec())), manifest, logo),
        OverlayState::Matches,
        "逐字节一致才算已加工好"
    );
    assert_eq!(
        archive::classify_overlay(
            Some((b"[app]\nversion = \"0.9.0\"\n".to_vec(), logo.to_vec())),
            manifest,
            logo
        ),
        OverlayState::Drift,
        "清单变了就是上一轮的产物"
    );
    // logo 单独变化同样算漂移 —— 它和清单一起进 overlay，只比清单会漏掉换图标那一类。
    assert_eq!(
        archive::classify_overlay(Some((manifest.to_vec(), b"PNG-B".to_vec())), manifest, logo),
        OverlayState::Drift,
        "只有 logo 变也是漂移"
    );
}
