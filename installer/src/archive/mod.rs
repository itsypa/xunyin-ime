pub mod format;
pub mod reader;
pub mod writer;

#[allow(unused_imports)]
pub use format::{
    ArchiveEntry, ArchiveFooter, ArchiveHeader, CompressionType, FORMAT_VERSION, MAGIC_FOOTER,
    MAGIC_HEADER,
};
pub use reader::ArchiveReader;
#[allow(unused_imports)]
pub use writer::ArchiveWriter;

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

/// 将「仅含清单 + logo、无文件」的最小归档作为 overlay 追加到已存在的 exe 末尾。
///
/// 用于让卸载器（被解压到安装目录的裸 stub）自包含运行期清单：安装时调用本函数把
/// 清单/logo 追加到 install_dir\uninstall.exe，卸载器启动即可用 `ArchiveReader::
/// open_current_exe` 从自身读取——安装目录无需任何额外散落文件。
///
/// 追加结构：`[原 exe][Header(manifest, logo, 0 条目, 块大小 0)][Footer]`。
/// 无压缩块（0 个文件），运行期只读头部、不解压。
pub fn append_manifest_overlay(
    exe_path: &Path,
    manifest: &[u8],
    logo: &[u8],
) -> Result<(), String> {
    use format::{ArchiveFooter, ArchiveHeader, CompressionType};

    let stub_size = std::fs::metadata(exe_path)
        .map_err(|e| format!("Failed to stat exe: {}", e))?
        .len();

    let mut header = ArchiveHeader::new(CompressionType::Zstd);
    header.manifest = manifest.to_vec();
    header.logo = logo.to_vec();
    header.entry_count = 0;
    header.solid_compressed_size = 0; // 无压缩块
    let header_bytes = header.to_bytes();

    // Header 紧接在原 exe 之后，故 header_offset = 原 exe 大小
    let footer = ArchiveFooter::new(stub_size);

    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(exe_path)
        .map_err(|e| format!("Failed to open exe for append: {}", e))?;
    f.write_all(&header_bytes)
        .map_err(|e| format!("Failed to append header: {}", e))?;
    f.write_all(&footer.to_bytes())
        .map_err(|e| format!("Failed to append footer: {}", e))?;
    Ok(())
}

/// 读回 exe 尾部 overlay 里的 `(清单, logo)` 字节；没有 overlay（或清单为空）返回 `None`。
///
/// 打包器用它回答一个比「有没有 overlay」更细的问题：**这个 overlay 是不是本轮配置产的**。
/// 两者只在「一次干净构建」里等价，源目录留着上一轮产物时就分道扬镳 —— 而那种情况下
/// 已加工的文件**不能就地更新**（重写 PE 会毁掉可能已经打上的签名），只能报错。
///
/// `ArchiveReader::open` 自带证书表偏移解析，已签名的文件同样读得出。任何打不开的
/// 情况（裸 stub、非 PE、损坏）都返回 `None` —— 「读不出 overlay」与「没有 overlay」
/// 在这里是同一个决定：当作未加工，补一次。
pub fn read_manifest_overlay(exe_path: &Path) -> Option<(Vec<u8>, Vec<u8>)> {
    let r = ArchiveReader::open(exe_path).ok()?;
    let manifest = r.manifest_bytes().to_vec();
    if manifest.is_empty() {
        return None;
    }
    Some((manifest, r.logo_bytes().to_vec()))
}

/// 源目录里那个 `uninstall.exe` 相对于**本轮配置**处于什么状态。
///
/// 抽成纯函数是为了能把三条决策路径钉在测试里 —— 它们各自对应一种截然不同的处置
/// （加工 / 跳过 / 报错），而走错任何一条的后果都是静默的坏包。
// 只有打包器用得到；`archive` 在 bin 与 lib 里各编一份，bin 那份用不到它
// （同 `bundle_exe`）。
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub enum OverlayState {
    /// 没有 overlay：未加工的裸 stub。
    Absent,
    /// overlay 与本轮配置逐字节一致：已经加工好了。
    Matches,
    /// overlay 存在但内容对不上：**上一轮**的产物。
    Drift,
}

#[allow(dead_code)]
pub fn classify_overlay(
    existing: Option<(Vec<u8>, Vec<u8>)>,
    manifest: &[u8],
    logo: &[u8],
) -> OverlayState {
    match existing {
        None => OverlayState::Absent,
        Some((have_manifest, have_logo)) => {
            if have_manifest == manifest && have_logo == logo {
                OverlayState::Matches
            } else {
                OverlayState::Drift
            }
        }
    }
}

/// exe 尾部是否已带清单 overlay。
///
/// 安装期用：那里只需要知道「要不要补追加」，不关心 overlay 的内容是哪一版 ——
/// 装机端本就没有第二份配置可比。而幂等在这里不是锦上添花，是**保护签名**：
/// overlay 现在由打包器在构建机上追加、随后立刻签名，装机端若再追加一次，证书表就
/// 不再是文件最后一段，Authenticode 直接判定为「没有签名」。
pub fn has_manifest_overlay(exe_path: &Path) -> bool {
    read_manifest_overlay(exe_path).is_some()
}

/// 将 stub EXE 和归档 .bin 捆绑为最终安装程序。
///
/// Solid 格式下 entry.offset 是解压后流中的偏移，与 stub 大小无关，
/// 因此 Header 字节可原样复制，只需更新 Footer 中的 header_offset。
///
/// .bin 布局：[compressed_block: N bytes][header_bytes: H bytes][footer: 16 bytes]
///   footer.header_offset = N
///
/// 最终 .exe 布局：[stub: S bytes][compressed_block: N bytes][header_bytes: H bytes][footer: 16 bytes]
///   footer.header_offset = S + N
#[allow(dead_code)]
pub fn bundle_exe(stub_path: &Path, archive_path: &Path, output_path: &Path) -> Result<(), String> {
    let stub_size = std::fs::metadata(stub_path)
        .map_err(|e| format!("Failed to stat stub: {}", e))?
        .len();

    let arc_reader = ArchiveReader::open(archive_path)?;
    let solid_size = arc_reader.header().solid_compressed_size;

    let archive_size = std::fs::metadata(archive_path)
        .map_err(|e| format!("Failed to stat archive: {}", e))?
        .len();

    // header_bytes 在 .bin 中占的字节数（compressed_block 与 footer 之间）
    let header_bytes_size = archive_size - solid_size - 16;

    let mut out = BufWriter::new(
        std::fs::File::create(output_path)
            .map_err(|e| format!("Failed to create output: {}", e))?,
    );

    let mut arc_src = BufReader::new(
        std::fs::File::open(archive_path).map_err(|e| format!("Failed to open archive: {}", e))?,
    );

    // 1. stub
    let mut stub_src = BufReader::new(
        std::fs::File::open(stub_path).map_err(|e| format!("Failed to open stub: {}", e))?,
    );
    std::io::copy(&mut stub_src, &mut out).map_err(|e| format!("Failed to write stub: {}", e))?;

    // 2. 压缩块（原样复制，无需任何修改）
    let mut block_src = (&mut arc_src).take(solid_size);
    std::io::copy(&mut block_src, &mut out)
        .map_err(|e| format!("Failed to write compressed block: {}", e))?;

    // 3. Header 字节（原样复制，entry.offset 不依赖 stub 大小）
    let mut header_src = (&mut arc_src).take(header_bytes_size);
    std::io::copy(&mut header_src, &mut out)
        .map_err(|e| format!("Failed to write header: {}", e))?;

    // 4. 更新后的 Footer（只有 header_offset 变化：加上 stub_size）
    let new_footer = ArchiveFooter::new(stub_size + solid_size);
    out.write_all(&new_footer.to_bytes())
        .map_err(|e| format!("Failed to write footer: {}", e))?;
    out.flush().ok();

    Ok(())
}
