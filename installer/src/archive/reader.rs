use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

static BACKUP_SEQ: AtomicU32 = AtomicU32::new(0);

use super::format::{ArchiveEntry, ArchiveFooter, ArchiveHeader};
use crate::archive::CompressionType;

/// 归档数据的逻辑末尾偏移 —— Footer 的右边界，不一定是文件的物理末尾。
///
/// Authenticode 代码签名把证书表**追加在 PE 文件末尾**，并把它的文件偏移与长度写进
/// Optional Header 的第 5 个数据目录项（`IMAGE_DIRECTORY_ENTRY_SECURITY`）。于是一个
/// 签过名的自解压 exe 布局变成：
///
/// ```text
///   [stub][压缩块][Header][Footer 16B][证书表]
///                              ↑           ↑
///                         archive_end   file_len
/// ```
///
/// 若仍按物理末尾往回读 16 字节，读到的是证书表的尾巴，magic 校验必然失败
/// （"Invalid footer magic"）—— 也就是「安装包一签名就打不开」。故这里先把证书表
/// 的起始偏移解析出来当作归档末尾。
///
/// ⚠️ `IMAGE_DIRECTORY_ENTRY_SECURITY` 是全部 16 个数据目录项里**唯一**一个
/// `VirtualAddress` 存的是文件偏移而非 RVA 的特例，可以直接 seek，不需要按节表换算。
///
/// 未签名文件、以及任何解析不下去的情况（非 PE、目录项缺失、数值不自洽），一律回落到
/// `file_len`，行为与加入本函数之前完全一致。
fn archive_end(file: &mut File, file_len: u64) -> Result<u64, String> {
    /// PE 头最小可解析长度：e_lfanew(0x3C) 之后还要够 PE 签名 + COFF 头 + 可选头。
    const MIN_PE_LEN: u64 = 0x40;

    if file_len < MIN_PE_LEN {
        return Ok(file_len);
    }

    fn read_u32(file: &mut File, off: u64) -> Option<u32> {
        file.seek(SeekFrom::Start(off)).ok()?;
        let mut b = [0u8; 4];
        file.read_exact(&mut b).ok()?;
        Some(u32::from_le_bytes(b))
    }
    fn read_u16(file: &mut File, off: u64) -> Option<u16> {
        file.seek(SeekFrom::Start(off)).ok()?;
        let mut b = [0u8; 2];
        file.read_exact(&mut b).ok()?;
        Some(u16::from_le_bytes(b))
    }

    // DOS 头 e_lfanew → PE 签名 "PE\0\0"
    let pe_off = match read_u32(file, 0x3C) {
        Some(v) => v as u64,
        None => return Ok(file_len),
    };
    if pe_off + 24 + 112 > file_len {
        return Ok(file_len);
    }
    if read_u32(file, pe_off) != Some(0x0000_4550) {
        return Ok(file_len); // 不是 PE，按裸文件处理
    }

    // Optional Header 的 Magic 决定数据目录的起点：PE32 偏移 96，PE32+ 偏移 112。
    let opt_off = pe_off + 24;
    let dd_off = match read_u16(file, opt_off) {
        Some(0x10B) => opt_off + 96,  // PE32
        Some(0x20B) => opt_off + 112, // PE32+
        _ => return Ok(file_len),
    };

    // NumberOfRvaAndSizes 紧邻数据目录之前，必须 > 4 才有 Security 项（索引 4）。
    if read_u32(file, dd_off - 4).is_none_or(|n| n <= 4) {
        return Ok(file_len);
    }

    let sec_off = dd_off + 4 * 8;
    let (cert_off, cert_size) = match (read_u32(file, sec_off), read_u32(file, sec_off + 4)) {
        (Some(o), Some(s)) => (o as u64, s as u64),
        _ => return Ok(file_len),
    };

    // 判据：证书表必须恰好占满文件尾部。Authenticode 本就要求如此（尾部多一个字节，
    // 签名即被判定为 "No signature found"），故这个等式不成立就说明目录项不可信，
    // 宁可回落到物理末尾也不要拿一个错的偏移去读 Footer。
    if cert_off == 0 || cert_size == 0 || cert_off >= file_len || cert_off + cert_size != file_len {
        return Ok(file_len);
    }

    // ⚠️ Footer 未必紧挨着证书表：证书表要求 8 字节对齐，signtool 会在原文件末尾补
    // 0..=7 字节填充再追加它。故归档末尾是 cert_off 减去那段填充，而填充长度不写在
    // 任何头里，只能按 magic 反查。
    //
    // 实测（本仓 21997457 字节的 Setup.exe，21997457 % 8 == 1 → 补 7 字节）：
    //   cert_off-16 处读到 00 57494e44454e4400 00000000000000
    //                       ↑ "WINDEND\0" 落在 [1..9] 而非 [8..16]
    // 若不补这一步，只有「原大小恰为 8 的倍数」的包能打开，其余七分之六在签名后照样
    // 报 Invalid footer magic —— 而且是随构建产物大小随机复现，最难查的那种。
    for pad in 0..=7u64 {
        let end = match cert_off.checked_sub(pad) {
            Some(v) if v >= 16 => v,
            _ => break,
        };
        if file.seek(SeekFrom::Start(end - 8)).is_err() {
            break;
        }
        let mut m = [0u8; 8];
        if file.read_exact(&mut m).is_err() {
            break;
        }
        if &m == super::format::MAGIC_FOOTER {
            return Ok(end);
        }
    }

    // 证书表在、但它前面找不到 Footer：这不是我们打的包（例如一个普通的已签名 exe）。
    // 回落物理末尾，让后续的 magic 校验给出「不是归档」这个正确结论。
    Ok(file_len)
}

/// 归档读取器（Solid 压缩 v2）
///
/// 打开时仅解析 Header/Footer，不立即解压。首次调用 `prepare()` 或 `extract_entry()`
/// 时将整个压缩块一次性解压到内存（`decompressed` 缓冲），后续每个文件的提取只是
/// 从缓冲中切片写盘，速度极快。
pub struct ArchiveReader {
    file: File,
    header: ArchiveHeader,
    /// 压缩块在文件中的绝对起始偏移 = footer.header_offset - solid_compressed_size
    block_start: u64,
    /// 懒解压缓冲：首次 prepare() 后填充
    decompressed: Option<Vec<u8>>,
}

#[allow(dead_code)]
impl ArchiveReader {
    /// 打开当前运行的 EXE 文件
    pub fn open_current_exe() -> Result<Self, String> {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("Failed to get current exe path: {}", e))?;
        Self::open(&exe_path)
    }

    /// 打开指定文件并解析归档元数据（不解压数据块）
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| format!("Failed to open file: {}", e))?;

        let file_len = file
            .metadata()
            .map_err(|e| format!("Failed to get file size: {}", e))?
            .len();

        if file_len < 16 {
            return Err("File too small to contain footer".into());
        }

        // 归档数据的末尾未必是文件的物理末尾：Authenticode 签名会把证书表追加在 PE
        // 之后，Footer 于是被顶到证书表前面（详见 archive_end）。
        let archive_end = archive_end(&mut file, file_len)?;
        if archive_end < 16 {
            return Err("File too small to contain footer".into());
        }

        // 读取尾部 Footer（归档末尾的 16 字节）
        file.seek(SeekFrom::Start(archive_end - 16))
            .map_err(|e| format!("Failed to seek to footer: {}", e))?;
        let mut footer_bytes = [0u8; 16];
        file.read_exact(&mut footer_bytes)
            .map_err(|e| format!("Failed to read footer: {}", e))?;
        let footer = ArchiveFooter::from_bytes(&footer_bytes)?;

        // 读取 Header（footer.header_offset 到 footer 之间）
        let header_size = archive_end
            .checked_sub(16)
            .and_then(|v| v.checked_sub(footer.header_offset))
            .ok_or("Invalid archive: header_size underflow")?;
        file.seek(SeekFrom::Start(footer.header_offset))
            .map_err(|e| format!("Failed to seek to header: {}", e))?;
        let mut header_bytes = vec![0u8; header_size as usize];
        file.read_exact(&mut header_bytes)
            .map_err(|e| format!("Failed to read header: {}", e))?;
        let header = ArchiveHeader::from_bytes(&header_bytes)?;

        // 压缩块起始 = footer.header_offset - solid_compressed_size
        let block_start = footer
            .header_offset
            .checked_sub(header.solid_compressed_size)
            .ok_or("Invalid archive: block_start underflow")?;

        Ok(Self {
            file,
            header,
            block_start,
            decompressed: None,
        })
    }

    pub fn header(&self) -> &ArchiveHeader {
        &self.header
    }
    pub fn entries(&self) -> &[ArchiveEntry] {
        &self.header.entries
    }
    pub fn compression_type(&self) -> CompressionType {
        self.header.compression
    }

    /// 运行期清单字节（AppManifest 的 TOML 文本），无解压开销。
    pub fn manifest_bytes(&self) -> &[u8] {
        &self.header.manifest
    }
    /// UI logo 图片字节，无解压开销。
    pub fn logo_bytes(&self) -> &[u8] {
        &self.header.logo
    }

    /// 提前将压缩块整体解压到内存缓冲。
    /// 安装向导在进入逐文件循环前调用此方法，可在独立步骤中显示解压进度消息。
    pub fn prepare(&mut self) -> Result<(), String> {
        if self.decompressed.is_none() {
            self.decompress_block()?;
        }
        Ok(())
    }

    /// 从已解压缓冲中提取单个文件到目标路径
    pub fn extract_entry(&mut self, entry: &ArchiveEntry, dest: &Path) -> Result<(), String> {
        if self.decompressed.is_none() {
            self.decompress_block()?;
        }

        let start = entry.offset as usize;
        let size = entry.original_size as usize;
        let expected_crc = entry.crc32;
        let path_str = entry.path.clone();

        // 创建目标目录
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create dir for {}: {}", path_str, e))?;
        }

        // 切片写盘（borrow 限定在此块内）
        {
            let dec = self.decompressed.as_ref().unwrap();
            let end = start + size;
            if end > dec.len() {
                return Err(format!(
                    "Entry '{}' out of bounds in decompressed stream (offset={} size={} total={})",
                    path_str,
                    start,
                    size,
                    dec.len()
                ));
            }
            let slice = &dec[start..end];

            // 验证 CRC32
            let computed = crc32fast::hash(slice);
            if computed != expected_crc {
                return Err(format!(
                    "CRC32 mismatch for '{}': expected {:08x}, got {:08x}",
                    path_str, expected_crc, computed
                ));
            }

            // 写文件（被占用时先改名备份）
            let mut output = BufWriter::new(create_or_backup(dest)?);
            output
                .write_all(slice)
                .map_err(|e| format!("Failed to write '{}': {}", path_str, e))?;
        }

        Ok(())
    }

    /// 将所有文件解压到目标目录（一次性解压块，逐文件写盘）
    pub fn extract_all(&mut self, dest_dir: &Path) -> Result<(), String> {
        self.prepare()?;
        let entries = self.header.entries.clone();
        for entry in &entries {
            let dest = dest_dir.join(&entry.path);
            self.extract_entry(entry, &dest)?;
        }
        Ok(())
    }

    /// 将整个压缩块读入内存并解压
    fn decompress_block(&mut self) -> Result<(), String> {
        self.file
            .seek(SeekFrom::Start(self.block_start))
            .map_err(|e| format!("Failed to seek to compressed block: {}", e))?;

        let compressed_size = self.header.solid_compressed_size as usize;
        let mut compressed = vec![0u8; compressed_size];
        self.file
            .read_exact(&mut compressed)
            .map_err(|e| format!("Failed to read compressed block: {}", e))?;

        let decompressed = match self.header.compression {
            CompressionType::Zstd => zstd::decode_all(&compressed[..])
                .map_err(|e| format!("Zstd decompression failed: {}", e))?,
            CompressionType::Lzma => {
                let mut decoder = xz2::read::XzDecoder::new(&compressed[..]);
                let mut out = Vec::new();
                decoder
                    .read_to_end(&mut out)
                    .map_err(|e| format!("LZMA decompression failed: {}", e))?;
                out
            }
        };

        self.decompressed = Some(decompressed);
        Ok(())
    }
}

/// 创建输出文件；若目标被其他进程占用（os error 5 / 32），将其改名为 .old_<seq> 后重试
fn create_or_backup(path: &Path) -> Result<File, String> {
    match File::create(path) {
        Ok(f) => return Ok(f),
        Err(ref e) if matches!(e.raw_os_error(), Some(5) | Some(32)) => {
            let seq = BACKUP_SEQ.fetch_add(1, Ordering::Relaxed);
            let suffix = std::process::id().wrapping_add(seq);
            let old_name = format!(
                "{}.old_{:08x}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                suffix
            );
            let old_path = path.parent().unwrap_or(Path::new(".")).join(old_name);
            std::fs::rename(path, &old_path)
                .map_err(|e| format!("Failed to rename locked file: {}", e))?;
            // 改名只是让路：那份旧文件仍被进程占用、当前删不掉，排进重启删除队列兜底，
            // 否则 .old_ 会随每次带锁升级永久累积。best-effort——失败不影响新文件写入。
            //
            // 门控到 Windows：`util` 是 cfg(windows) 模块，而 archive 必须保持跨平台
            // ——wind-packer 要在 Linux 原生构建（见 lib.rs 与 ci.yml 的 linux-packer job）。
            // 非 Windows 上这个分支本就走不到：进入它的条件是 os error 5/32，即 Windows 的
            // ACCESS_DENIED / SHARING_VIOLATION，类 Unix 系统允许 unlink 仍被打开的文件。
            #[cfg(windows)]
            if let Err(e) = crate::util::reboot::schedule_delete_on_reboot(&old_path) {
                eprintln!("Warning: {}", e);
            }
        }
        Err(e) => return Err(format!("Failed to create output file: {}", e)),
    }
    File::create(path).map_err(|e| format!("Failed to create file after backup: {}", e))
}
