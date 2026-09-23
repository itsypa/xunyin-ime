// ArchiveWriter 仅 wind-packer 与测试使用；installer/uninstaller 二进制不构造它。
#![allow(dead_code)]

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use super::format::{ArchiveEntry, ArchiveFooter, ArchiveHeader, CompressionType, MAGIC_HEADER};

/// 归档写入器（Solid 压缩）
///
/// 先将所有文件数据收集到内存，`finish()` 时将全部原始数据拼成一条流整体压缩一次，
/// 再写入 Header 和 Footer。相比逐文件压缩，压缩器可跨文件复用字典，对含大量
/// 相似文件（词典 YAML 等）的归档压缩率提升显著。
pub struct ArchiveWriter {
    compression: CompressionType,
    output: File,
    /// (archive_path, raw_data)
    pending: Vec<(String, Vec<u8>)>,
    /// 运行期安装配置（AppManifest 的 TOML 文本字节）
    manifest: Vec<u8>,
    /// UI 显示的 logo 图片字节
    logo: Vec<u8>,
}

impl ArchiveWriter {
    pub fn new(output_path: &Path, compression: CompressionType) -> Result<Self, String> {
        let output = File::create(output_path)
            .map_err(|e| format!("Failed to create output file: {}", e))?;
        Ok(Self {
            compression,
            output,
            pending: Vec::new(),
            manifest: Vec::new(),
            logo: Vec::new(),
        })
    }

    /// 设置运行期清单与 logo，写入归档头部。
    pub fn set_manifest(&mut self, manifest: Vec<u8>, logo: Vec<u8>) {
        self.manifest = manifest;
        self.logo = logo;
    }

    /// 将文件加入待压缩队列（暂存到内存，不立即写盘）
    pub fn add_file(&mut self, source_path: &Path, archive_path: &str) -> Result<(), String> {
        let mut f = File::open(source_path)
            .map_err(|e| format!("Failed to open {}: {}", source_path.display(), e))?;
        let mut data = Vec::new();
        f.read_to_end(&mut data)
            .map_err(|e| format!("Failed to read {}: {}", source_path.display(), e))?;
        self.pending.push((archive_path.to_string(), data));
        Ok(())
    }

    /// 递归添加目录
    pub fn add_directory(&mut self, dir_path: &Path, base_path: &str) -> Result<(), String> {
        for entry in std::fs::read_dir(dir_path)
            .map_err(|e| format!("Failed to read directory {}: {}", dir_path.display(), e))?
        {
            let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            let archive_path = if base_path.is_empty() {
                file_name.clone()
            } else {
                format!("{}/{}", base_path, file_name)
            };

            if path.is_dir() {
                self.add_directory(&path, &archive_path)?;
            } else {
                self.add_file(&path, &archive_path)?;
            }
        }
        Ok(())
    }

    /// 固实压缩并写入归档，返回 Header 在文件中的偏移（= solid_compressed_size）
    pub fn finish(mut self) -> Result<u64, String> {
        // 1. 构建 entry 表，同时将所有原始数据拼接为一条流
        let mut combined: Vec<u8> = Vec::new();
        let mut entries: Vec<ArchiveEntry> = Vec::new();
        let mut decompressed_offset: u64 = 0;

        for (path, data) in &self.pending {
            let original_size = data.len() as u64;
            let crc32 = crc32fast::hash(data);
            entries.push(ArchiveEntry {
                path: path.clone(),
                offset: decompressed_offset, // 解压后流中的字节起点
                compressed_size: 0,          // solid 模式无意义
                original_size,
                crc32,
            });
            combined.extend_from_slice(data);
            decompressed_offset += original_size;
        }

        // 2. 整体压缩一次
        let compressed = compress_solid(&combined, self.compression)?;
        let solid_compressed_size = compressed.len() as u64;

        // 3. 写入压缩块
        self.output
            .write_all(&compressed)
            .map_err(|e| format!("Failed to write compressed block: {}", e))?;

        // 4. 构建并写入 Header
        let mut header = ArchiveHeader::new(self.compression);
        header.magic = *MAGIC_HEADER;
        header.manifest = std::mem::take(&mut self.manifest);
        header.logo = std::mem::take(&mut self.logo);
        header.entry_count = entries.len() as u32;
        header.solid_compressed_size = solid_compressed_size;
        header.entries = entries;

        let header_bytes = header.to_bytes();
        let header_offset = solid_compressed_size; // .bin 中 Header 从此处开始
        self.output
            .write_all(&header_bytes)
            .map_err(|e| format!("Failed to write header: {}", e))?;

        // 5. 写入 Footer
        let footer = ArchiveFooter::new(header_offset);
        self.output
            .write_all(&footer.to_bytes())
            .map_err(|e| format!("Failed to write footer: {}", e))?;

        Ok(header_offset)
    }

    pub fn entry_count(&self) -> u32 {
        self.pending.len() as u32
    }
}

fn compress_solid(data: &[u8], compression: CompressionType) -> Result<Vec<u8>, String> {
    match compression {
        CompressionType::Zstd => {
            // 多线程压缩。产物仍是标准 zstd 流 —— 解压端（reader.rs 的 zstd::decode_all）
            // 一行不用改：多线程只是把输入切块并行压，帧格式完全兼容。
            //
            // 为什么值得改：打包是发布流程里最大的一块（实测占 d8 的 43%、44.8 s），
            // 而 encode_all 是单线程的，在多核编译机上只用得到 1 个核。切块会让压缩率
            // 损失不到 1%，换来接近线性的加速。
            let workers = std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1);
            let t0 = std::time::Instant::now();
            let mut enc = zstd::Encoder::new(Vec::new(), 19)
                .map_err(|e| format!("Zstd encoder init failed: {}", e))?;
            // 单核机器上不启用 —— 保持与从前逐字节相同的产物。
            if workers > 1 {
                enc.multithread(workers)
                    .map_err(|e| format!("Zstd multithread setup failed: {}", e))?;
                // 不设 JobSize 的话默认块很大，几十 MB 的输入只切得出两三块，于是再多核
                // 也只有两三个 worker 在干活（实测 48 核峰值仅 ~2.7 核）。8 MB 一块，
                // 50 MB 级的产物能摊到 6~7 个 worker；块内仍是 solid，跨块才失去字典复用，
                // 压缩率损失可控。
                enc.set_parameter(zstd::stream::raw::CParameter::JobSize(8 << 20))
                    .map_err(|e| format!("Zstd job size setup failed: {}", e))?;
            }
            enc.write_all(data)
                .map_err(|e| format!("Zstd write failed: {}", e))?;
            let out = enc
                .finish()
                .map_err(|e| format!("Zstd finish failed: {}", e))?;
            // 打包是发布流程里最大的一块，把它的耗时和压缩比直接打出来 —— 否则下次想优化
            // 又得靠外部采样去猜它占多少。
            eprintln!(
                "    [zstd] {:.1} MB → {:.1} MB  {:.1}s  ({} 线程)",
                data.len() as f64 / 1048576.0,
                out.len() as f64 / 1048576.0,
                t0.elapsed().as_secs_f64(),
                workers
            );
            Ok(out)
        }
        CompressionType::Lzma => {
            let mut encoder = xz2::write::XzEncoder::new(Vec::new(), 9);
            encoder
                .write_all(data)
                .map_err(|e| format!("LZMA write failed: {}", e))?;
            encoder
                .finish()
                .map_err(|e| format!("LZMA finish failed: {}", e))
        }
    }
}
