//! Wind Packer —— 通用安装器生成工具。
//!
//! 由 `app.toml` 单一配置驱动：应用身份、打包文件、品牌（logo/icon）全部在其中描述。
//! 同一个预编译 stub（wind-installer.exe）配不同 app.toml 即可生成不同应用的安装包，
//! 无需重新编译。
//!
//! 子命令：
//! - `prep-uninstaller` 把源目录里的 uninstall.exe 加工成终态（版本信息 + 图标 + 清单
//!   overlay）。`pack` 会在需要时自己做这一步，单独暴露只为给代码签名腾位置：
//!   卸载器必须在**进归档之前**签，签完就不能再被改动。
//! - `pack`   压缩源目录为 .bin（含 manifest + logo，慢，仅需一次）
//! - `bundle` 给 stub 写图标后拼接 .bin 为安装程序（快，可反复执行）
//! - `build`  pack + bundle 一步到位

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use wind_installer::archive::{self, ArchiveWriter, CompressionType};
use wind_installer::manifest::ProjectConfig;

mod version_info;

#[derive(Parser, Debug)]
#[command(name = "wind-packer")]
#[command(about = "Generic installer generator driven by app.toml")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 压缩源目录为 .bin 归档（嵌入 manifest 与 logo）
    Pack {
        /// app.toml 配置文件
        #[arg(short, long, default_value = "app.toml")]
        config: PathBuf,
        /// 版本号（覆盖 app.toml 中的 app.version，同时影响嵌入清单与默认输出文件名）
        #[arg(short = 'V', long)]
        version: Option<String>,
        /// 源目录（覆盖 app.toml 中的 package.source_dir；相对路径以当前工作目录为基准）
        #[arg(short = 'd', long)]
        source_dir: Option<PathBuf>,
        /// 压缩算法（覆盖 app.toml 中的 package.compression：lzma | zstd）
        #[arg(short = 'z', long)]
        compression: Option<String>,
        /// 输出 .bin（默认 <output_dir>/<output_name>-<version>.bin）
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 要求源目录里的 uninstall.exe **已经**加工过（配合 `prep-uninstaller` 使用）
        ///
        /// 不给这个开关时，遇到未加工的卸载器会就地补做。但当调用方在 prep 与打包之间
        /// 夹了一次代码签名，补做出来的卸载器是**没签名**的，而调用方以为签过了 ——
        /// 给上这个开关就把那种情形变成报错。
        #[arg(long)]
        require_prepared_uninstaller: bool,
    },
    /// 给 stub 写图标后拼接 .bin 为最终安装程序
    Bundle {
        /// app.toml 配置文件
        #[arg(short, long, default_value = "app.toml")]
        config: PathBuf,
        /// 版本号（覆盖 app.toml 中的 app.version，影响默认 archive / output 文件名）
        #[arg(short = 'V', long)]
        version: Option<String>,
        /// stub 文件（wind-installer.exe）
        #[arg(short, long)]
        stub: PathBuf,
        /// 输入 .bin（默认 <output_dir>/<output_name>-<version>.bin）
        #[arg(short, long)]
        archive: Option<PathBuf>,
        /// 输出安装程序（默认 <output_dir>/<output_name>-<version>.exe）
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// pack + bundle 一步到位
    Build {
        /// app.toml 配置文件
        #[arg(short, long, default_value = "app.toml")]
        config: PathBuf,
        /// 版本号（覆盖 app.toml 中的 app.version，同时影响嵌入清单与默认输出文件名）
        #[arg(short = 'V', long)]
        version: Option<String>,
        /// 源目录（覆盖 app.toml 中的 package.source_dir；相对路径以当前工作目录为基准）
        #[arg(short = 'd', long)]
        source_dir: Option<PathBuf>,
        /// 压缩算法（覆盖 app.toml 中的 package.compression：lzma | zstd）
        #[arg(short = 'z', long)]
        compression: Option<String>,
        /// stub 文件（wind-installer.exe）
        #[arg(short, long)]
        stub: PathBuf,
        /// 输出安装程序（默认 <output_dir>/<output_name>-<version>.exe）
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 要求源目录里的 uninstall.exe **已经**加工过（配合 `prep-uninstaller` 使用）
        ///
        /// 不给这个开关时，遇到未加工的卸载器会就地补做。但当调用方在 prep 与打包之间
        /// 夹了一次代码签名，补做出来的卸载器是**没签名**的，而调用方以为签过了 ——
        /// 给上这个开关就把那种情形变成报错。
        #[arg(long)]
        require_prepared_uninstaller: bool,
    },
    /// 把源目录里的 uninstall.exe 加工成终态（版本信息 + 图标 + 清单 overlay）
    ///
    /// 单独暴露这一步，是为了给代码签名腾出位置：`prep-uninstaller` → 签名 → `pack`。
    /// 卸载器一旦签名就不能再被任何人改动，而 `pack` 之后它已经封进压缩块，补签够不着。
    ///
    /// 同一份配置下幂等，可重复调用；配置变了则报错而非重做 —— 已加工的文件不能就地
    /// 更新（重写 PE 会毁签名），只能拿未加工的 stub 覆盖后重来。
    PrepUninstaller {
        /// app.toml 配置文件
        #[arg(short, long, default_value = "app.toml")]
        config: PathBuf,
        /// 版本号（覆盖 app.toml 中的 app.version）
        #[arg(short = 'V', long)]
        version: Option<String>,
        /// 源目录（覆盖 app.toml 中的 package.source_dir；相对路径以当前工作目录为基准）
        #[arg(short = 'd', long)]
        source_dir: Option<PathBuf>,
    },
    /// 读取已生成的安装程序（或 .bin），打印其嵌入的清单摘要
    Inspect {
        /// 安装程序 .exe 或归档 .bin
        #[arg(short, long)]
        file: PathBuf,
    },
}

fn main() {
    let args = Args::parse();
    let result = match args.command {
        Command::Pack {
            config,
            version,
            source_dir,
            compression,
            output,
            require_prepared_uninstaller,
        } => {
            let ov = Overrides {
                version,
                source_dir,
                compression,
            };
            cmd_pack(&config, ov, output, require_prepared_uninstaller).map(|_| ())
        }
        Command::Bundle {
            config,
            version,
            stub,
            archive,
            output,
        } => {
            let ov = Overrides {
                version,
                source_dir: None,
                compression: None,
            };
            cmd_bundle(&config, ov, &stub, archive, output)
        }
        Command::Build {
            config,
            version,
            source_dir,
            compression,
            stub,
            output,
            require_prepared_uninstaller,
        } => {
            let ov = Overrides {
                version,
                source_dir,
                compression,
            };
            cmd_build(&config, ov, &stub, output, require_prepared_uninstaller)
        }
        Command::PrepUninstaller {
            config,
            version,
            source_dir,
        } => {
            let ov = Overrides {
                version,
                source_dir,
                compression: None,
            };
            cmd_prep_uninstaller(&config, ov)
        }
        Command::Inspect { file } => cmd_inspect(&file),
    };
    if let Err(e) = result {
        eprintln!("错误: {}", e);
        std::process::exit(1);
    }
}

// ── CLI 覆盖项 ───────────────────────────────────────────────────────────────

/// CLI 传入的覆盖值；`None` 表示"沿用 app.toml 中的值"。
struct Overrides {
    version: Option<String>,
    /// 源目录（来自 CLI，相对路径以 CWD 为基准）
    source_dir: Option<PathBuf>,
    compression: Option<String>,
}

// ── 配置加载与路径解析 ──────────────────────────────────────────────────────

struct Loaded {
    cfg: ProjectConfig,
    /// app.toml 所在目录，相对路径以此为基准
    base: PathBuf,
}

fn load(config: &Path) -> Result<Loaded, String> {
    let text = std::fs::read_to_string(config)
        .map_err(|e| format!("读取配置 {:?} 失败: {}", config, e))?;
    let cfg = ProjectConfig::from_toml_str(&text)?;
    let base = config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(Loaded { cfg, base })
}

impl Loaded {
    /// 应用 CLI 覆盖项（在 `load` 之后立即调用）。
    ///
    /// `source_dir` 来自命令行时相对于 CWD 解析为绝对路径，
    /// 使后续 `resolve()` 对绝对路径直接透传，不再 join config base dir。
    fn apply_overrides(&mut self, ov: Overrides) {
        if let Some(v) = ov.version {
            self.cfg.manifest.app.version = v;
        }
        if let Some(s) = ov.source_dir {
            let abs = if s.is_absolute() {
                s
            } else {
                std::env::current_dir().unwrap_or_default().join(s)
            };
            self.cfg.package.source_dir = abs.to_string_lossy().into_owned();
        }
        if let Some(c) = ov.compression {
            self.cfg.package.compression = c;
        }
    }

    /// 相对 app.toml 解析路径
    fn resolve(&self, p: &str) -> PathBuf {
        let path = Path::new(p);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.base.join(path)
        }
    }

    fn version(&self) -> &str {
        &self.cfg.manifest.app.version
    }

    fn output_base(&self) -> String {
        let name = if self.cfg.package.output_name.is_empty() {
            self.cfg.manifest.app.id.clone()
        } else {
            self.cfg.package.output_name.clone()
        };
        format!("{}-{}", name, self.version())
    }

    fn default_bin(&self) -> PathBuf {
        self.resolve(&self.cfg.package.output_dir)
            .join(format!("{}.bin", self.output_base()))
    }

    fn default_exe(&self) -> PathBuf {
        self.resolve(&self.cfg.package.output_dir)
            .join(format!("{}.exe", self.output_base()))
    }

    /// 运行期清单（TOML 文本字节）。
    fn manifest_bytes(&self) -> Result<Vec<u8>, String> {
        self.cfg.manifest.to_toml_bytes()
    }

    /// logo 字节（可选）。读不到只警告并回退到空——stub 内置了默认 logo，
    /// 这里失败不该让整次打包中断。
    fn logo_bytes(&self) -> Vec<u8> {
        if self.cfg.package.logo.is_empty() {
            return Vec::new();
        }
        let logo_path = self.resolve(&self.cfg.package.logo);
        match std::fs::read(&logo_path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!(
                    "警告: 无法读取 logo {:?}: {}（将使用空 logo）",
                    logo_path, e
                );
                Vec::new()
            }
        }
    }

    /// 打包期需要写进 PE 的图标路径；`package.icon` 为空表示不写图标。
    fn icon_path(&self) -> Result<Option<PathBuf>, String> {
        if self.cfg.package.icon.is_empty() {
            return Ok(None);
        }
        let p = self.resolve(&self.cfg.package.icon);
        if !p.exists() {
            return Err(format!("图标文件不存在: {:?}", p));
        }
        Ok(Some(p))
    }
}

// ── prep-uninstaller ────────────────────────────────────────────────────────

/// 把 `<source>/uninstall.exe` 从裸 stub 加工成**终态**卸载器：写版本信息与图标，
/// 再把清单 overlay 追加到尾部。
///
/// 为什么必须在打包期做完，而不是像早先那样留到安装期在用户机器上追加：
/// Authenticode 要求证书表是 PE 的最后一段，尾部多一个字节签名即失效。只要装机端
/// 还会改这个文件，它就永远签不了。而 overlay 的内容全部来自 `app.toml`，本就没有
/// 任何安装期输入，前移不丢信息。前移之后的顺序是
/// `prep-uninstaller` → 签名 → 进归档 → 装机端只解压，签名一路完好。
///
/// **本函数幂等**，但判据问的不是「有没有 overlay」而是「overlay 是不是**本轮配置**产的」。
/// 这两件事只在「一次干净构建」里等价：源目录留着上一轮的产物时（`wind-packer build`
/// 自己不删 `uninstall.exe`，只有 `pack.ps1` 的 `finally` 删），只问前者就会把**上一版**
/// 的卸载器原样封进包 —— 包能装、验签也过，只有卸载器按旧清单走，是最难发现的那种坏包。
/// 配置对不上时只能报错、不能覆盖：文件可能已经签名，重写 PE 就把签名毁了。
///
/// 判据也要挡在「写版本信息」和「追加 overlay」两件事**之前**，不能夹在中间：
/// `set_pe_version_info` 改的是 PE 内容，签名当场作废；而且资源节放不下时 editpe 会
/// **新增一个节**插在尾部数据之前，把已有 overlay 整体后移，Footer 里记的绝对偏移
/// 随即对不上，归档从此打不开（见 `docs/DESIGN.md` 打包管线那段）。
fn prepare_uninstaller(
    l: &Loaded,
    source: &Path,
    require_prepared: bool,
) -> Result<PrepOutcome, String> {
    let uninstaller_path = source.join("uninstall.exe");
    if !uninstaller_path.exists() {
        // 声明「卸载器应当已经加工好了」而它压根不在，多半是 source_dir 配错了。
        // 放过去就打出一个**没有卸载入口**的包 —— 比失败更糟，而且装完才发现。
        if require_prepared {
            return Err(format!(
                "{:?} 不存在，但本次调用要求卸载器已经加工好。\n\
                 请核对 package.source_dir 是否指向真正的产物目录。",
                uninstaller_path
            ));
        }
        return Ok(PrepOutcome::NoUninstaller);
    }

    let manifest = l.manifest_bytes()?;
    let logo = l.logo_bytes();

    match archive::classify_overlay(
        archive::read_manifest_overlay(&uninstaller_path),
        &manifest,
        &logo,
    ) {
        archive::OverlayState::Matches => {
            println!("  卸载器已按当前配置加工过，跳过 —— 重复加工会毁掉已有签名");
            return Ok(PrepOutcome::AlreadyPrepared);
        }
        archive::OverlayState::Drift => {
            return Err(format!(
                "{:?} 是**上一轮**的产物：它内嵌的清单/logo 与本次配置不一致。\n\
                 已加工的文件不能就地更新（重写 PE 会毁掉可能已经打上的签名），\n\
                 请先用未加工的 stub 覆盖它再重试（pack.ps1 每轮会自动做这件事）。",
                uninstaller_path
            ));
        }
        // 调用方声明「卸载器应当已经加工好了」时，就地补加工是错的：那说明前一步的
        // prep 根本没跑到（或跑失败了），而**签名夹在 prep 与本步之间** —— 补加工出来的
        // 卸载器是没签名的，调用方却以为签过了。这正是本次改动要消灭的那类静默失败。
        archive::OverlayState::Absent if require_prepared => {
            return Err(format!(
                "{:?} 还是未加工的 stub，但本次调用要求它已经加工好。\n\
                 这通常意味着前一步的 prep-uninstaller 没有跑成功 —— 就地补加工会\n\
                 得到一个【没签名】的卸载器（签名夹在 prep 与打包之间），故拒绝继续。",
                uninstaller_path
            ));
        }
        archive::OverlayState::Absent => {}
    }

    let uninst_res_info = version_info::derive_version_info(&l.cfg, true);
    println!("  加工卸载程序:");
    println!("    描述:     {}", uninst_res_info.file_description);
    println!("    文件版本: {}", uninst_res_info.file_version);

    let icon_path = l.icon_path()?;
    set_pe_version_info(&uninstaller_path, &uninst_res_info, icon_path.as_deref())?;

    archive::append_manifest_overlay(&uninstaller_path, &manifest, &logo)?;
    let size = std::fs::metadata(&uninstaller_path)
        .map(|m| m.len())
        .unwrap_or(0);
    println!("    → 版本信息与清单 overlay 已写入（{} 字节）", size);
    Ok(PrepOutcome::Prepared)
}

/// [`prepare_uninstaller`] 的三种结局。「没有加工对象」对 `pack` 是正常的（普通应用
/// 可以不带卸载器），对 `prep-uninstaller` 却是失败 —— 那是个专门用来加工卸载器的
/// 子命令，没有对象就说明路径配错了。区分开才能让后者报错而不是静默退 0。
#[derive(Debug, PartialEq, Eq)]
enum PrepOutcome {
    Prepared,
    AlreadyPrepared,
    NoUninstaller,
}

fn cmd_prep_uninstaller(config: &Path, ov: Overrides) -> Result<(), String> {
    let mut l = load(config)?;
    l.apply_overrides(ov);
    let source = l.resolve(&l.cfg.package.source_dir);
    if !source.exists() {
        return Err(format!("源目录不存在: {:?}", source));
    }
    println!("Wind Packer · prep-uninstaller");
    println!("  源目录: {:?}", source);
    if prepare_uninstaller(&l, &source, false)? == PrepOutcome::NoUninstaller {
        // 退 0 会让「路径配错了」一路无人报警：调用方（pack.ps1 -PrepOnly）接着去签
        // 一个不存在的文件，而签名脚本对不存在的目标也是静默放过的。
        return Err(format!("源目录内没有 uninstall.exe: {:?}", source));
    }
    Ok(())
}

// ── pack ────────────────────────────────────────────────────────────────────

fn cmd_pack(
    config: &Path,
    ov: Overrides,
    output: Option<PathBuf>,
    require_prepared: bool,
) -> Result<PathBuf, String> {
    let mut l = load(config)?;
    l.apply_overrides(ov);
    cmd_pack_inner(&l, output, require_prepared)
}

/// 对已加载并应用过覆盖项的 `Loaded` 执行打包。
/// `cmd_build` 复用此函数，避免二次加载配置文件。
fn cmd_pack_inner(
    l: &Loaded,
    output: Option<PathBuf>,
    require_prepared: bool,
) -> Result<PathBuf, String> {
    let output = output.unwrap_or_else(|| l.default_bin());
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir).ok();
    }

    let source = l.resolve(&l.cfg.package.source_dir);
    if !source.exists() {
        return Err(format!("源目录不存在: {:?}", source));
    }

    let compression = match l.cfg.package.compression.to_lowercase().as_str() {
        "lzma" | "xz" => CompressionType::Lzma,
        _ => CompressionType::Zstd,
    };

    let manifest_bytes = l.manifest_bytes()?;
    let logo_bytes = l.logo_bytes();

    println!("Wind Packer · pack");
    println!(
        "  应用:   {} {}",
        l.cfg.manifest.app.display_name,
        l.version()
    );
    println!("  源目录: {:?}", source);
    println!("  压缩:   {:?}", compression);
    println!("  清单:   {} 字节", manifest_bytes.len());
    println!("  logo:   {} 字节", logo_bytes.len());

    // 卸载器加工（版本信息 + 图标 + 清单 overlay）。
    //
    // 想签卸载器就必须**先 `prep-uninstaller`、签完再 pack**——签名夹在这两步之间，
    // 而这里已经来不及了：pack 会把它封进压缩块，之后再签只能签到外壳。
    // 这一句是给「不签名、直接 `wind-packer build`」那条路保底的：那时没人提前
    // prep 过，加工必须在这里发生，否则装出来的卸载器读不到清单、启动即失败。
    // 已按本轮配置 prep 过时函数自己跳过，不会碰已签名的文件；而 overlay 与本轮配置
    // **对不上**时（源目录留着上一轮的产物）它返回 `Err`，整次 build 就此失败 ——
    // 那种情况下静默沿用旧卸载器比失败更糟。
    prepare_uninstaller(l, &source, require_prepared)?;

    let mut writer = ArchiveWriter::new(&output, compression)?;
    writer.set_manifest(manifest_bytes, logo_bytes);
    writer.add_directory(&source, "")?;
    let entry_count = writer.entry_count();
    writer.finish()?;

    let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    println!(
        "  → {:?}（{} 个文件, {:.2} MB）",
        output,
        entry_count,
        size as f64 / 1048576.0
    );
    Ok(output)
}

// ── bundle ──────────────────────────────────────────────────────────────────

fn cmd_bundle(
    config: &Path,
    ov: Overrides,
    stub: &Path,
    archive: Option<PathBuf>,
    output: Option<PathBuf>,
) -> Result<(), String> {
    let mut l = load(config)?;
    l.apply_overrides(ov);
    let archive = archive.unwrap_or_else(|| l.default_bin());
    let output = output.unwrap_or_else(|| l.default_exe());
    bundle_inner(&l, stub, &archive, &output)
}

fn bundle_inner(l: &Loaded, stub: &Path, archive: &Path, output: &Path) -> Result<(), String> {
    if !stub.exists() {
        return Err(format!("stub 不存在: {:?}", stub));
    }
    if !archive.exists() {
        return Err(format!("归档不存在: {:?}（请先 pack）", archive));
    }
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir).ok();
    }

    println!("Wind Packer · bundle");

    // 推导安装器的版本详细信息
    let res_info = version_info::derive_version_info(&l.cfg, false);
    println!("  注入版本信息:");
    println!("    产品名称: {}", res_info.product_name);
    println!("    描述:     {}", res_info.file_description);
    println!("    文件版本: {}", res_info.file_version);
    println!("    公司:     {}", res_info.company_name);

    let tmp = output.with_extension("stub.tmp");
    let icon_path = if l.cfg.package.icon.is_empty() {
        None
    } else {
        let p = l.resolve(&l.cfg.package.icon);
        if p.exists() {
            Some(p)
        } else {
            return Err(format!("图标文件不存在: {:?}", p));
        }
    };

    // 复制 stub 到临时文件然后注入 PE 资源
    std::fs::copy(stub, &tmp).map_err(|e| format!("复制 stub 失败: {}", e))?;
    set_pe_version_info(&tmp, &res_info, icon_path.as_deref())?;

    let effective_stub = tmp.clone();
    let iconned_stub = Some(tmp);

    let stub_size = std::fs::metadata(&effective_stub)
        .map(|m| m.len())
        .unwrap_or(0);
    archive::bundle_exe(&effective_stub, archive, output)?;

    if let Some(tmp) = iconned_stub {
        let _ = std::fs::remove_file(&tmp);
    }

    let total = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    println!(
        "  → {:?}（stub={} 字节, 总计 {:.2} MB）",
        output,
        stub_size,
        total as f64 / 1048576.0
    );
    Ok(())
}

// ── build（pack + bundle）───────────────────────────────────────────────────

fn cmd_build(
    config: &Path,
    ov: Overrides,
    stub: &Path,
    output: Option<PathBuf>,
    require_prepared: bool,
) -> Result<(), String> {
    let mut l = load(config)?;
    l.apply_overrides(ov);
    let output = output.unwrap_or_else(|| l.default_exe());
    let bin = cmd_pack_inner(&l, None, require_prepared)?;
    bundle_inner(&l, stub, &bin, &output)
}

// ── inspect ─────────────────────────────────────────────────────────────────

fn cmd_inspect(file: &Path) -> Result<(), String> {
    use wind_installer::archive::ArchiveReader;
    use wind_installer::manifest::AppManifest;

    let reader = ArchiveReader::open(file)?;
    println!("文件:     {:?}", file);
    println!("文件数:   {}", reader.entries().len());
    println!("压缩:     {:?}", reader.compression_type());
    println!("logo:     {} 字节", reader.logo_bytes().len());

    let mb = reader.manifest_bytes();
    if mb.is_empty() {
        println!("清单:     （无——该归档不含运行期清单，无法作为安装器使用）");
        return Ok(());
    }
    let m = AppManifest::from_toml_bytes(mb)?;
    println!("清单:     {} 字节", mb.len());
    println!("  id            = {}", m.app.id);
    println!("  display_name  = {}", m.app.display_name);
    println!("  version       = {}", m.app.version);
    println!("  main_exe      = {}", m.app.main_exe);

    // 能力段：缺省即该能力不执行，故必须逐项列出「关」的那些——
    // 一份漏写 [autostart] 的清单打出来的包不会自启，而这在打包期是静默的。
    println!("能力:");
    print_capability("输入法注册 [ime]", m.ime.is_some(), || {
        m.ime
            .as_ref()
            .map(|i| format!("lang_id={}", i.lang_id))
            .unwrap_or_default()
    });
    print_capability("字体安装 [[font]]", !m.font.is_empty(), || {
        format!("{} 项", m.font.len())
    });
    print_capability(
        "开机自启 [autostart]",
        m.autostart.as_ref().is_some_and(|a| a.enabled),
        || {
            m.autostart
                .as_ref()
                .map(|a| a.exe_or(&m.app.main_exe).to_string())
                .unwrap_or_default()
        },
    );
    print_capability("快捷方式 [[shortcut]]", !m.shortcut.is_empty(), || {
        // 展开占位符显示实际快捷方式名（与运行期安装器一致），便于核对
        m.shortcut
            .iter()
            .map(|s| {
                wind_installer::manifest::expand_placeholders(
                    s.effective_name(),
                    &m.app.main_exe,
                    &m.app.setting_exe,
                    &m.app.display_name,
                    &m.app.id,
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    });
    print_capability(
        "装完启动 [startup]",
        m.startup.as_ref().is_some_and(|s| s.prestart),
        || {
            m.startup
                .as_ref()
                .map(|s| s.exe_or(&m.app.main_exe).to_string())
                .unwrap_or_default()
        },
    );
    print_capability("数据目录配置 [datadir]", m.datadir.is_some(), || {
        m.datadir
            .as_ref()
            .map(|d| d.conf_file.clone())
            .unwrap_or_default()
    });
    print_capability(
        "本机数据清理 [localdata]",
        m.localdata.is_some(),
        || {
            m.localdata
                .as_ref()
                .map(|l| {
                    format!(
                        "缓存 {} 项 / 状态 {} 项",
                        l.cache_dirs.len(),
                        l.state_files.len()
                    )
                })
                .unwrap_or_default()
        },
    );
    print_capability("URL 协议", !m.app.url_protocol.trim().is_empty(), || {
        format!("{}://", m.app.url_protocol)
    });

    Ok(())
}

/// 打印一项能力的开关状态。关闭的能力也要显示——静默缺省正是最难发现的配置错误。
fn print_capability(name: &str, enabled: bool, detail: impl FnOnce() -> String) {
    if enabled {
        println!("  [✓] {:<22} {}", name, detail());
    } else {
        println!("  [ ] {:<22} （未声明，不执行）", name);
    }
}

// ── PE 版本与图标注入 ────────────────────────────────────────────────────────

/// 为指定 EXE 注入版本信息和图标（可选）
fn set_pe_version_info(
    exe_path: &Path,
    res_info: &version_info::ResolvedVersionInfo,
    icon_path: Option<&Path>,
) -> Result<(), String> {
    let mut image =
        editpe::Image::parse_file(exe_path).map_err(|e| format!("解析 PE 失败: {}", e))?;
    let mut resources = image.resource_directory().cloned().unwrap_or_default();

    // 1. 读取或创建 VersionInfo
    let mut version_info = resources
        .get_version_info()
        .map_err(|e| format!("获取 VersionInfo 失败: {}", e))?
        .unwrap_or_default();

    // 2. 设置 FixedFileInfo
    let (f_major, f_minor) = version_info::parse_version_string(&res_info.file_version);
    version_info.info.file_version = editpe::types::VersionU32 {
        major: f_major,
        minor: f_minor,
    };

    let (p_major, p_minor) = version_info::parse_version_string(&res_info.product_version);
    version_info.info.product_version = editpe::types::VersionU32 {
        major: p_major,
        minor: p_minor,
    };

    // 3. 设置语言和翻译段
    let lang_id = 0x0804u16; // 简体中文
    let code_page = 0x04b0u16; // Unicode
    version_info.vars = vec![editpe::types::VersionU16 {
        major: lang_id,
        minor: code_page,
    }];

    let table_key = format!("{:04x}{:04x}", lang_id, code_page);
    let mut strings_map = indexmap::IndexMap::default();
    strings_map.insert("CompanyName".to_string(), res_info.company_name.clone());
    strings_map.insert(
        "FileDescription".to_string(),
        res_info.file_description.clone(),
    );
    strings_map.insert("FileVersion".to_string(), res_info.file_version.clone());
    strings_map.insert("InternalName".to_string(), res_info.internal_name.clone());
    strings_map.insert("LegalCopyright".to_string(), res_info.copyright.clone());
    strings_map.insert(
        "OriginalFilename".to_string(),
        res_info.original_filename.clone(),
    );
    strings_map.insert("ProductName".to_string(), res_info.product_name.clone());
    strings_map.insert(
        "ProductVersion".to_string(),
        res_info.product_version.clone(),
    );

    version_info.strings = vec![editpe::VersionStringTable {
        key: table_key,
        strings: strings_map,
    }];

    resources
        .set_version_info(&version_info)
        .map_err(|e| format!("设置 VersionInfo 失败: {}", e))?;

    // 4. 设置图标（可选）
    if let Some(ico) = icon_path {
        if ico.exists() {
            resources
                .set_main_icon_file(&ico.to_string_lossy())
                .map_err(|e| format!("设置图标失败: {}", e))?;
        }
    }

    // 5. 写回资源
    image
        .set_resource_directory(resources)
        .map_err(|e| format!("写入资源目录失败: {}", e))?;
    image
        .write_file(exe_path)
        .map_err(|e| format!("写出 PE 失败: {}", e))?;
    Ok(())
}
