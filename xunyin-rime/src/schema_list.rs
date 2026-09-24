//! 方案清单维护：让「把新方案文件拷进来 + 重新部署」就能用（对标小狼毫）。
//!
//! Rime 只认 `default.yaml` / `default.custom.yaml` 里 `schema_list` 列出的方案；
//! 光把 `<id>.schema.yaml` 放进数据目录不会出现在方案列表里。小狼毫是靠部署
//! 界面让用户勾选，我们这里直接自动收录：重新部署时扫描共享目录与用户目录下的
//! 所有 `*.schema.yaml`，把还没在清单里的方案追加进去。

use std::path::Path;

/// 单个方案的信息。
#[derive(Debug, Clone)]
pub struct SchemaInfo {
    pub id: String,
    pub name: String,
}

/// 扫描目录里的 `*.schema.yaml`（只读头部若干行取 `schema_id` / `name`）。
fn scan_dir(dir: &Path, out: &mut Vec<SchemaInfo>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<std::path::PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .map(|n| {
                        let n = n.to_string_lossy();
                        n.ends_with(".schema.yaml") && !n.ends_with(".custom.yaml")
                    })
                    .unwrap_or(false)
        })
        .collect();
    files.sort();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let mut id: Option<String> = None;
        let mut name: Option<String> = None;
        for line in text.lines().take(80) {
            let t = line.trim();
            if id.is_none() {
                if let Some(v) = t.strip_prefix("schema_id:") {
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    if !v.is_empty() {
                        id = Some(v.to_string());
                    }
                }
            }
            if name.is_none() {
                if let Some(v) = t.strip_prefix("name:") {
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    if !v.is_empty() {
                        name = Some(v.to_string());
                    }
                }
            }
            if id.is_some() && name.is_some() {
                break;
            }
        }
        // 头部没写 schema_id 时退回文件名（`<id>.schema.yaml`）
        let id = id.or_else(|| {
            f.file_name()
                .map(|n| n.to_string_lossy().trim_end_matches(".schema.yaml").to_string())
        });
        let Some(id) = id.filter(|s| !s.is_empty()) else {
            continue;
        };
        if out.iter().any(|s| s.id == id) {
            continue;
        }
        out.push(SchemaInfo {
            id: id.clone(),
            name: name.unwrap_or(id),
        });
    }
}

/// 扫描共享目录 + 用户目录下已安装的方案（共享目录里的排在前面）。
pub fn discover(shared: &Path, user: &Path) -> Vec<SchemaInfo> {
    let mut out = Vec::new();
    scan_dir(shared, &mut out);
    scan_dir(user, &mut out);
    out
}

/// 把 `default.custom.yaml` 里已有的 schema_list 顺序取出来。
fn existing_order(user: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(user.join("default.custom.yaml")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("- schema:") {
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() && !out.iter().any(|x: &String| x == v) {
                out.push(v.to_string());
            }
        }
    }
    out
}

/// 重写用户目录的 `default.custom.yaml`：每页候选数 + 收录全部已安装方案。
///
/// 返回 Ok(true) 表示文件内容有变化。已有的方案顺序会保留，新发现的追加在后面。
pub fn sync_default_patch(shared: &Path, user: &Path, page_size: u32) -> std::io::Result<bool> {
    let found = discover(shared, user);
    let mut ids = existing_order(user);
    for s in &found {
        if !ids.iter().any(|x| *x == s.id) {
            ids.push(s.id.clone());
        }
    }
    // 一个都没扫到时，至少保住默认方案，别把 schema_list 写空
    if ids.is_empty() {
        ids.push("xunyin_rich".to_string());
    }
    let mut content = String::from("patch:\n");
    content.push_str(&format!("  menu/page_size: {}\n", page_size.clamp(1, 20)));
    content.push_str("  schema_list:\n");
    for id in &ids {
        content.push_str(&format!("    - schema: {id}\n"));
    }
    let path = user.join("default.custom.yaml");
    if std::fs::read_to_string(&path).map(|cur| cur == content).unwrap_or(false) {
        return Ok(false);
    }
    std::fs::create_dir_all(user)?;
    // 先写临时文件再改名：避免 librime 部署线程读到半截文件
    let tmp = user.join("default.custom.yaml.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}
