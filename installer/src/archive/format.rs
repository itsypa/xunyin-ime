/// 归档魔数
pub const MAGIC_HEADER: &[u8; 8] = b"WINDPKG\0";
pub const MAGIC_FOOTER: &[u8; 8] = b"WINDEND\0";

/// 当前格式版本
/// - v2: solid 压缩，entry.offset 为解压后流偏移
/// - v3: 头部新增 manifest 段（运行期安装配置）与 logo 段（UI 图片字节）
pub const FORMAT_VERSION: u32 = 3;

/// 压缩算法类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum CompressionType {
    /// Zstd - 解压速度快，内存可控
    Zstd = 0,
    /// LZMA - 压缩率最高
    Lzma = 1,
}

impl CompressionType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Zstd),
            1 => Some(Self::Lzma),
            _ => None,
        }
    }
}

/// 单个文件条目
///
/// v2 格式中：
/// - `offset`          = 该文件在**解压后数据流**中的起始字节位置（与文件在 EXE/BIN 中的位置无关）
/// - `compressed_size` = 0（固实压缩下无意义，整包只有一个压缩块）
/// - `original_size`   = 文件原始大小（解压后的字节数）
/// - `crc32`           = 原始文件数据的 CRC32
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArchiveEntry {
    pub path: String,
    pub offset: u64,
    pub compressed_size: u64,
    pub original_size: u64,
    pub crc32: u32,
}

/// 归档头部
///
/// 二进制布局（v3）：
/// ```text
/// [0..8]    magic "WINDPKG\0"
/// [8..12]   version u32 le
/// [12]      compression u8
/// [13..17]  manifest_len u32 le          ← v3 新增
/// [17..]    manifest_bytes（AppManifest 的 TOML 文本）
/// [+0..+4]  logo_len u32 le              ← v3 新增
/// [+4..]    logo_bytes（PNG，可为 0 字节）
/// [+0..+4]  entry_count u32 le
/// [+4..+12] solid_compressed_size u64 le
/// [+12..]   entry 列表
/// ```
///
/// manifest 与 logo 放在头部（未压缩），可在不解压固实块的前提下被运行期廉价读取。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArchiveHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub compression: CompressionType,
    /// 运行期安装配置（AppManifest 的 TOML 文本字节），可为空。
    pub manifest: Vec<u8>,
    /// UI 显示的 logo 图片字节（PNG），可为空。
    pub logo: Vec<u8>,
    pub entry_count: u32,
    /// 固实压缩块的字节数（紧接在 stub 之后、Header 之前）
    pub solid_compressed_size: u64,
    pub entries: Vec<ArchiveEntry>,
}

impl ArchiveHeader {
    pub fn new(compression: CompressionType) -> Self {
        Self {
            magic: *MAGIC_HEADER,
            version: FORMAT_VERSION,
            compression,
            manifest: Vec::new(),
            logo: Vec::new(),
            entry_count: 0,
            solid_compressed_size: 0,
            entries: Vec::new(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.magic);
        buf.extend_from_slice(&self.version.to_le_bytes());
        buf.push(self.compression as u8);

        buf.extend_from_slice(&(self.manifest.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.manifest);
        buf.extend_from_slice(&(self.logo.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.logo);

        buf.extend_from_slice(&self.entry_count.to_le_bytes());
        buf.extend_from_slice(&self.solid_compressed_size.to_le_bytes());

        for entry in &self.entries {
            let path_bytes = entry.path.as_bytes();
            buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(path_bytes);
            buf.extend_from_slice(&entry.offset.to_le_bytes());
            buf.extend_from_slice(&entry.compressed_size.to_le_bytes());
            buf.extend_from_slice(&entry.original_size.to_le_bytes());
            buf.extend_from_slice(&entry.crc32.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        // 最小长度：magic(8)+version(4)+compression(1)+manifest_len(4)+logo_len(4)
        //           +entry_count(4)+solid_size(8) = 33
        if data.len() < 33 {
            return Err("Header too short".into());
        }

        let mut magic = [0u8; 8];
        magic.copy_from_slice(&data[0..8]);
        if &magic != MAGIC_HEADER {
            return Err("Invalid header magic".into());
        }

        let version = u32::from_le_bytes(data[8..12].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(format!(
                "Unsupported format version: {} (expected {})",
                version, FORMAT_VERSION
            ));
        }

        let compression = CompressionType::from_u8(data[12])
            .ok_or_else(|| "Invalid compression type".to_string())?;

        let mut pos = 13;

        // manifest 段
        let read_blob = |data: &[u8], pos: &mut usize, what: &str| -> Result<Vec<u8>, String> {
            if *pos + 4 > data.len() {
                return Err(format!("Truncated {} length", what));
            }
            let len = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap()) as usize;
            *pos += 4;
            if *pos + len > data.len() {
                return Err(format!("Truncated {} bytes", what));
            }
            let blob = data[*pos..*pos + len].to_vec();
            *pos += len;
            Ok(blob)
        };
        let manifest = read_blob(data, &mut pos, "manifest")?;
        let logo = read_blob(data, &mut pos, "logo")?;

        if pos + 12 > data.len() {
            return Err("Truncated entry_count/solid_size".into());
        }
        let entry_count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        let solid_compressed_size = u64::from_le_bytes(data[pos + 4..pos + 12].try_into().unwrap());
        pos += 12;

        let mut entries = Vec::with_capacity(entry_count as usize);

        for _ in 0..entry_count {
            if pos + 2 > data.len() {
                return Err("Truncated entry path length".into());
            }
            let path_len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;

            if pos + path_len > data.len() {
                return Err("Truncated entry path".into());
            }
            let path = String::from_utf8(data[pos..pos + path_len].to_vec())
                .map_err(|_| "Invalid UTF-8 in path")?;
            pos += path_len;

            if pos + 28 > data.len() {
                return Err("Truncated entry fields".into());
            }
            let offset = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
            let compressed_size = u64::from_le_bytes(data[pos + 8..pos + 16].try_into().unwrap());
            let original_size = u64::from_le_bytes(data[pos + 16..pos + 24].try_into().unwrap());
            let crc32 = u32::from_le_bytes(data[pos + 24..pos + 28].try_into().unwrap());
            pos += 28;

            entries.push(ArchiveEntry {
                path,
                offset,
                compressed_size,
                original_size,
                crc32,
            });
        }

        Ok(Self {
            magic,
            version,
            compression,
            manifest,
            logo,
            entry_count,
            solid_compressed_size,
            entries,
        })
    }
}

/// 归档尾部（固定 16 字节，位于文件末尾）
#[derive(Debug, Clone)]
pub struct ArchiveFooter {
    /// Header 起始偏移（= stub_size + solid_compressed_size）
    pub header_offset: u64,
    pub magic: [u8; 8],
}

impl ArchiveFooter {
    pub fn new(header_offset: u64) -> Self {
        Self {
            header_offset,
            magic: *MAGIC_FOOTER,
        }
    }

    pub fn to_bytes(&self) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[0..8].copy_from_slice(&self.header_offset.to_le_bytes());
        buf[8..16].copy_from_slice(&self.magic);
        buf
    }

    pub fn from_bytes(data: &[u8; 16]) -> Result<Self, String> {
        let header_offset = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&data[8..16]);

        if &magic != MAGIC_FOOTER {
            return Err("Invalid footer magic".into());
        }

        Ok(Self {
            header_offset,
            magic,
        })
    }
}
