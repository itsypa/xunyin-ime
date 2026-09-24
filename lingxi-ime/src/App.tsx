import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as api from "./api";
import type {
  AppearanceConfig,
  DictStats,
  LingxiConfig,
  SchemaInfo,
} from "./api";
import "./App.css";

type Page = "input" | "appearance" | "dict" | "about";

const THEME_PRESETS: Record<string, Partial<AppearanceConfig>> = {
  light: {
    theme: "light",
    bg: "#FFFFFF",
    text: "#222222",
    accent: "#2A6FDB",
    selected_bg: "#2A6FDB",
    selected_text: "#FFFFFF",
    comment: "#999999",
    border: "#D8DCE3",
  },
  dark: {
    theme: "dark",
    bg: "#202127",
    text: "#E9E9EC",
    accent: "#5B93F2",
    selected_bg: "#3D6FBE",
    selected_text: "#FFFFFF",
    comment: "#8C919B",
    border: "#3B3D45",
  },
  green: {
    theme: "green",
    bg: "#F3F9F1",
    text: "#25311F",
    accent: "#3E8E41",
    selected_bg: "#3E8E41",
    selected_text: "#FFFFFF",
    comment: "#7C9A79",
    border: "#C6DCC2",
  },
};

const DEFAULT_CONFIG: LingxiConfig = {
  input: {
    schema: "lingxi_rich",
    full_shape: true,
    ascii_punct: false,
    ascii_mode: false,
    page_size: 5,
    beep: false,
    punct_map: [],
    enter_behavior: "commit",
    toggle_key: "shift",
    pinyin_separator: "'",
    fuzzy: [],
    enable_user_dict: true,
  },
  appearance: {
    font_size: 13,
    theme: "light",
    bg: "#FFFFFF",
    text: "#222222",
    accent: "#2A6FDB",
    selected_bg: "#2A6FDB",
    selected_text: "#FFFFFF",
    comment: "#999999",
    border: "#D8DCE3",
    corner_radius: 6,
    opacity: 100,
    show_index: true,
    show_status_bar: true,
  },
  dict: { custom_phrase_file: "custom_phrase.txt" },
  version: 1,
};

const PREVIEW_ITEMS = ["你", "您", "牛", "鸟", "捏"];

/** 模糊音组（id 必须与 lingxi-rime::config::FUZZY_GROUPS 一致）。 */
const FUZZY_GROUPS: { id: string; label: string }[] = [
  { id: "zh_z", label: "zh ↔ z" },
  { id: "ch_c", label: "ch ↔ c" },
  { id: "sh_s", label: "sh ↔ s" },
  { id: "n_l", label: "n ↔ l" },
  { id: "f_h", label: "f ↔ h" },
  { id: "r_l", label: "r ↔ l" },
  { id: "an_ang", label: "an ↔ ang" },
  { id: "en_eng", label: "en ↔ eng" },
  { id: "in_ing", label: "in ↔ ing" },
  { id: "ian_iang", label: "ian ↔ iang" },
  { id: "uan_uang", label: "uan ↔ uang" },
];

/** 回车键行为选项。 */
const ENTER_OPTIONS = [
  { value: "commit", label: "上屏输错的拼音（默认）" },
  { value: "clear", label: "丢弃当前输入" },
  { value: "newline", label: "丢弃当前输入并换行" },
];

/** 中英切换键选项。 */
const TOGGLE_KEY_OPTIONS = [
  { value: "shift", label: "Shift（默认）" },
  { value: "ctrl_space", label: "Ctrl + 空格" },
  { value: "none", label: "不切换（英文走候选/手动）" },
];

/** 拼音分隔符选项。 */
const SEPARATOR_OPTIONS = [
  { value: "'", label: "撇号 ' （ni'hao，默认）" },
  { value: "-", label: "连字符 - （ni-hao）" },
  { value: ",", label: "逗号 , （ni,hao）" },
  { value: " ", label: "空格 （ni hao）" },
  { value: "", label: "不显示分隔（nihao）" },
];

function App() {
  const [page, setPage] = useState<Page>("input");
  const [config, setConfigState] = useState<LingxiConfig>(DEFAULT_CONFIG);
  const [schemas, setSchemas] = useState<SchemaInfo[]>([]);
  const [stats, setStats] = useState<DictStats | null>(null);
  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState("");
  const [backups, setBackups] = useState<string[]>([]);
  const [selectedBackup, setSelectedBackup] = useState("");
  const [version, setVersion] = useState("0.1.0");
  const saveTimer = useRef<number | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const skinFileRef = useRef<HTMLInputElement>(null);

  const toastMsg = (msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast(""), 3200);
  };

  // 版本号以程序自身为准（构建时注入），避免与界面写死的数字不一致
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);

  // 初始化：引擎 → 配置 → 方案列表 → 词库统计
  useEffect(() => {
    (async () => {
      // 先加载配置，立即呈现界面
      try {
        const cfg = await api.getConfig();
        setConfigState(cfg);
      } catch (e) {
        toastMsg("读取配置失败：" + String(e));
      }
      setReady(true);
      // 后台初始化引擎（首次建索引可能较慢），完成后刷新方案与统计
      try {
        await api.initIme();
        for (let i = 0; i < 240; i++) {
          if (!(await api.isMaintenance())) break;
          await new Promise((r) => setTimeout(r, 500));
        }
        setSchemas(await api.listSchemas());
        setStats(await api.getDictStats());
      } catch (e) {
        toastMsg("引擎初始化失败：" + String(e));
      }
    })();
  }, []);

  // 配置变更 → 节流保存
  const saveConfig = useCallback((cfg: LingxiConfig, silent = false) => {
    setConfigState(cfg);
    if (saveTimer.current) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(async () => {
      try {
        await api.setConfig(cfg);
        if (!silent) toastMsg("设置已保存");
      } catch (e) {
        toastMsg("保存失败：" + String(e));
      }
    }, 350);
  }, []);

  const patchInput = (patch: Partial<LingxiConfig["input"]>) =>
    saveConfig({ ...config, input: { ...config.input, ...patch } });

  // 用户词库备份 / 恢复
  const refreshBackups = async () => {
    try {
      const list = await api.listUserDictBackups();
      setBackups(list);
      if (!selectedBackup && list.length) setSelectedBackup(list[0]);
    } catch (e) {
      toastMsg("读取备份列表失败：" + String(e));
    }
  };

  const handleBackupDict = async () => {
    setBusy(true);
    try {
      const dir = await api.backupUserDict();
      toastMsg("已备份用户词库到：" + dir);
      await refreshBackups();
    } catch (e) {
      toastMsg("备份失败：" + String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleRestoreDict = async () => {
    if (!selectedBackup) {
      toastMsg("请先选择一个备份");
      return;
    }
    setBusy(true);
    try {
      const msg = await api.restoreUserDict(selectedBackup);
      toastMsg(msg + "，正在重建索引…");
      await api.redeploy();
      setStats(await api.getDictStats());
      toastMsg("用户词库已恢复");
    } catch (e) {
      toastMsg("恢复失败：" + String(e));
    } finally {
      setBusy(false);
    }
  };

  // 进入词库页时刷新备份列表
  useEffect(() => {
    if (page === "dict") void refreshBackups();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page]);

  /** 从磁盘重新载入配置（外部改过配置文件时用）。 */
  const handleReload = async () => {
    try {
      setConfigState(await api.getConfig());
      toastMsg("已从磁盘重新载入配置");
    } catch (e) {
      toastMsg("重载失败：" + String(e));
    }
  };

  // ---- 标点自定义 ----
  const punctList = config.input.punct_map ?? [];
  const patchPunct = (list: { key: string; out: string }[]) =>
    saveConfig({ ...config, input: { ...config.input, punct_map: list } });
  const addPunct = () => patchPunct([...punctList, { key: "", out: "" }]);
  const updatePunct = (i: number, field: "key" | "out", value: string) => {
    const list = punctList.map((p, idx) => (idx === i ? { ...p, [field]: value } : p));
    patchPunct(list);
  };
  const removePunct = (i: number) => {
    const list = punctList.filter((_, idx) => idx !== i);
    patchPunct(list);
  };

  // ---- 皮肤导入导出 ----
  const handleExportSkin = async () => {
    setBusy(true);
    try {
      toastMsg("皮肤已导出：" + (await api.exportSkin()));
    } catch (e) {
      toastMsg("导出失败：" + String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleImportSkin = async (file?: File | null) => {
    if (!file) return;
    setBusy(true);
    try {
      const cfg = await api.importSkin(await file.text());
      setConfigState(cfg);
      toastMsg("皮肤已导入并套用");
    } catch (e) {
      toastMsg("导入失败：" + String(e));
    } finally {
      setBusy(false);
    }
  };

  const patchAppearance = (patch: Partial<AppearanceConfig>) =>
    saveConfig({ ...config, appearance: { ...config.appearance, ...patch } }, true);

  // 诊断与维护
  const handleOpenDataDir = async () => {
    try {
      await api.openDataDir();
    } catch (e) {
      toastMsg("打开失败：" + String(e));
    }
  };

  const handleOpenLogDir = async () => {
    try {
      await api.openLogDir();
    } catch (e) {
      toastMsg("打开失败：" + String(e));
    }
  };

  const handleClearLogs = async () => {
    try {
      const freed = await api.clearLogs();
      toastMsg(`已清空日志（释放 ${Math.round(freed / 1024)} KB）`);
    } catch (e) {
      toastMsg("清空失败：" + String(e));
    }
  };

  const applyPreset = (key: string) => {
    const preset = THEME_PRESETS[key];
    if (!preset) return;
    saveConfig({ ...config, appearance: { ...config.appearance, ...preset } });
    toastMsg("主题已切换");
  };

  const handleImport = async (file: File | undefined) => {
    if (!file) return;
    const text = await file.text();
    setBusy(true);
    try {
      const s = await api.importDictContent(file.name, text);
      setStats(s);
      toastMsg(s.last_import || "导入完成");
      await api.redeploy();
      toastMsg("词库已导入，正在重建索引…");
    } catch (e) {
      toastMsg("导入失败：" + String(e));
    } finally {
      setBusy(false);
      if (fileRef.current) fileRef.current.value = "";
    }
  };

  const handleExport = async () => {
    setBusy(true);
    try {
      const path = await api.exportDict();
      toastMsg("已导出到：" + path);
    } catch (e) {
      toastMsg("导出失败：" + String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleRedeploy = async () => {
    setBusy(true);
    try {
      await api.redeploy();
      toastMsg("正在重建词库索引，请稍候…");
      for (let i = 0; i < 600; i++) {
        if (!(await api.isMaintenance())) break;
        await new Promise((r) => setTimeout(r, 500));
      }
      setStats(await api.getDictStats());
      toastMsg("词库索引已更新");
    } catch (e) {
      toastMsg("重建失败：" + String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleReset = async () => {
    setBusy(true);
    try {
      const cfg = await api.resetConfig();
      setConfigState(cfg);
      toastMsg("已恢复默认设置，正在重建词库索引…");
      for (let i = 0; i < 600; i++) {
        if (!(await api.isMaintenance())) break;
        await new Promise((r) => setTimeout(r, 500));
      }
      setStats(await api.getDictStats());
    } catch (e) {
      toastMsg("恢复默认失败：" + String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!ready) {
    return (
      <div className="splash">
        <div className="splash-logo">灵</div>
        <div className="splash-title">灵犀输入法</div>
        <div className="splash-sub">正在初始化词库索引…</div>
        <div className="splash-bar">
          <div className="splash-bar-inner" />
        </div>
      </div>
    );
  }

  const a = config.appearance;

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-logo">灵</div>
          <div>
            <div className="brand-name">灵犀输入法</div>
            <div className="brand-sub">极致输入体验</div>
          </div>
        </div>
        <nav className="nav">
          <NavItem active={page === "input"} icon="⌨" label="输入设置" onClick={() => setPage("input")} />
          <NavItem active={page === "appearance"} icon="◐" label="外观主题" onClick={() => setPage("appearance")} />
          <NavItem active={page === "dict"} icon="☰" label="词库管理" onClick={() => setPage("dict")} />
          <NavItem active={page === "about"} icon="ⓘ" label="关于" onClick={() => setPage("about")} />
        </nav>
        <div className="sidebar-foot">
          {busy && <span className="badge busy">处理中…</span>}
          {stats && <span className="badge">词库 {stats.table_size_mb.toFixed(0)} MB</span>}
        </div>
      </aside>

      <main className="content">
        {/* 全局操作栏：设置自动保存，这里放跨页的通用操作 */}
        <div className="actionbar">
          <span className="actionbar-status">
            设置自动保存 · 约 1 秒后对新会话生效
          </span>
          <div className="actionbar-btns">
            <button className="btn small" disabled={busy} onClick={handleReload}>
              重载配置
            </button>
            <button className="btn small" disabled={busy} onClick={handleRedeploy}>
              重建词库索引
            </button>
            <button className="btn small" disabled={busy} onClick={handleReset}>
              恢复默认
            </button>
          </div>
        </div>
        {page === "input" && (
          <section className="page">
            <h1>输入设置</h1>
            <p className="page-desc">以下设置对新开的输入会话生效。</p>

            <div className="card">
              <h2>默认输入方案</h2>
              <select
                className="select"
                value={config.input.schema}
                onChange={(e) => patchInput({ schema: e.target.value })}
              >
                {schemas.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
              </select>
            </div>

            <div className="card">
              <h2>输入模式</h2>
              <Row
                label="全角 / 半角"
                desc="切换标点与字符的占位宽度"
                control={
                  <Toggle
                    checked={config.input.full_shape}
                    labels={["半角", "全角"]}
                    onChange={(v) => patchInput({ full_shape: v })}
                  />
                }
              />
              <Row
                label="中英标点"
                desc="中文标点（，。）与英文标点（, .），可用 Ctrl+. 快速切换"
                control={
                  <Toggle
                    checked={config.input.ascii_punct}
                    labels={["中文", "英文"]}
                    onChange={(v) => patchInput({ ascii_punct: v })}
                  />
                }
              />
              <Row
                label="默认中英模式"
                desc="开启后新会话直接输入英文"
                control={
                  <Toggle
                    checked={config.input.ascii_mode}
                    labels={["中文", "英文"]}
                    onChange={(v) => patchInput({ ascii_mode: v })}
                  />
                }
              />
            </div>

            <div className="card">
              <h2>标点自定义</h2>
              <p className="hint">
                仅对**中文标点**模式生效：把某个按键改成上屏你指定的文本
                （例如把 “ 键改成「）。留空即沿用 Rime 内置标点表；组合中按该键会先上屏拼音，
                再上屏自定义标点。
              </p>
              {punctList.length === 0 && (
                <p className="hint" style={{ marginTop: 8 }}>
                  暂无自定义（使用内置标点表）。
                </p>
              )}
              {punctList.map((p, i) => (
                <div className="row" key={i} style={{ marginTop: 8 }}>
                  <input
                    className="input"
                    style={{ width: 72, textAlign: "center" }}
                    maxLength={1}
                    placeholder="按键"
                    value={p.key}
                    onChange={(e) => updatePunct(i, "key", e.target.value)}
                  />
                  <span className="row-desc" style={{ margin: "0 8px" }}>
                    →
                  </span>
                  <input
                    className="input"
                    style={{ flex: 1 }}
                    placeholder="上屏文本（可多字）"
                    value={p.out}
                    onChange={(e) => updatePunct(i, "out", e.target.value)}
                  />
                  <button
                    className="btn small"
                    style={{ marginLeft: 8 }}
                    onClick={() => removePunct(i)}
                  >
                    删除
                  </button>
                </div>
              ))}
              <div className="btn-row" style={{ marginTop: 10 }}>
                <button className="btn small" onClick={addPunct}>
                  添加一条
                </button>
              </div>
            </div>

            <div className="card">
              <h2>候选窗</h2>
              <Row
                label="每页候选数"
                desc="调整后自动重建词库索引"
                control={
                  <Slider
                    min={3}
                    max={10}
                    value={config.input.page_size}
                    suffix=" 个"
                    onChange={(v) => patchInput({ page_size: v })}
                  />
                }
              />
            </div>

            <div className="card">
              <h2>按键行为</h2>
              <Row
                label="回车键"
                desc="组合中按回车的行为（空格始终用于选词）"
                control={
                  <select
                    className="select"
                    style={{ minWidth: 210 }}
                    value={config.input.enter_behavior}
                    onChange={(e) => patchInput({ enter_behavior: e.target.value })}
                  >
                    {ENTER_OPTIONS.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                }
              />
              <Row
                label="中英切换键"
                desc="未组合时按该键在中文/英文之间切换"
                control={
                  <select
                    className="select"
                    style={{ minWidth: 210 }}
                    value={config.input.toggle_key}
                    onChange={(e) => patchInput({ toggle_key: e.target.value })}
                  >
                    {TOGGLE_KEY_OPTIONS.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                }
              />
              <Row
                label="拼音分隔符"
                desc="组合串里音节之间的显示符号（不影响候选）"
                control={
                  <select
                    className="select"
                    style={{ minWidth: 210 }}
                    value={config.input.pinyin_separator}
                    onChange={(e) => patchInput({ pinyin_separator: e.target.value })}
                  >
                    {SEPARATOR_OPTIONS.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                }
              />
            </div>

            <div className="card">
              <h2>模糊音</h2>
              <p className="hint">
                勾选后不标准读音也能出字（如勾上 zh↔z，输入 zongguo 也能出「中国」）。
                保存后会自动重建索引，对新输入会话生效。
              </p>
              <div className="fuzzy-grid">
                {FUZZY_GROUPS.map((g) => (
                  <label className="fuzzy-item" key={g.id}>
                    <input
                      type="checkbox"
                      checked={(config.input.fuzzy ?? []).includes(g.id)}
                      onChange={(e) => {
                        const cur = config.input.fuzzy ?? [];
                        const next = e.target.checked
                          ? [...cur, g.id]
                          : cur.filter((x) => x !== g.id);
                        patchInput({ fuzzy: next });
                      }}
                    />
                    <span>{g.label}</span>
                  </label>
                ))}
              </div>
            </div>

            <div className="card">
              <h2>词库学习</h2>
              <Row
                label="用户词典自学习"
                desc="记住你常选的词并调整词频（关闭后不再学习新词）"
                control={
                  <Toggle
                    checked={config.input.enable_user_dict}
                    labels={["关闭", "开启"]}
                    onChange={(v) => patchInput({ enable_user_dict: v })}
                  />
                }
              />
            </div>
          </section>
        )}

        {page === "appearance" && (
          <section className="page">
            <h1>外观主题</h1>
            <p className="page-desc">自定义候选窗的样式与显示效果。</p>

            <div className="card">
              <h2>主题预设</h2>
              <div className="preset-row">
                {Object.keys(THEME_PRESETS).map((key) => (
                  <button
                    key={key}
                    className={`preset ${a.theme === key ? "active" : ""}`}
                    onClick={() => applyPreset(key)}
                  >
                    <span
                      className="preset-dot"
                      style={{
                        background:
                          THEME_PRESETS[key].selected_bg ?? "#2A6FDB",
                      }}
                    />
                    {key === "light" ? "浅色" : key === "dark" ? "深色" : "护眼"}
                  </button>
                ))}
              </div>
            </div>

            <div className="card">
              <h2>候选窗预览</h2>
              <p className="hint" style={{ marginTop: 0 }}>
                这里演示配色、圆角、序号与透明度（固定尺寸，不随字号变化）。
                字号请把鼠标停在下方「字体大小」滑块上，会弹出真实大小的演示。
              </p>
              <CandidatePreview config={config} />
            </div>

            <div className="card">
              <h2>显示</h2>
              <Row
                label="字体大小"
                desc="候选文字的显示字号（鼠标停在滑块上可弹出等大演示）"
                control={
                  <FontSizeSlider
                    min={9}
                    max={26}
                    value={a.font_size}
                    config={config}
                    onChange={(v) => patchAppearance({ font_size: v })}
                  />
                }
              />
              <Row
                label="窗口不透明度"
                desc="候选窗的透明程度"
                control={
                  <Slider
                    min={30}
                    max={100}
                    value={a.opacity}
                    suffix=" %"
                    onChange={(v) => patchAppearance({ opacity: v })}
                  />
                }
              />
              <Row
                label="显示序号"
                desc="候选词前的数字编号"
                control={
                  <Toggle
                    checked={a.show_index}
                    labels={["隐藏", "显示"]}
                    onChange={(v) => patchAppearance({ show_index: v })}
                  />
                }
              />
              <Row
                label="显示状态栏"
                desc="右下角悬浮条（中/英、标点、全/半角、符号、设置）"
                control={
                  <Toggle
                    checked={a.show_status_bar}
                    labels={["隐藏", "显示"]}
                    onChange={(v) => patchAppearance({ show_status_bar: v })}
                  />
                }
              />
            </div>

            <div className="card">
              <h2>自定义颜色</h2>
              <div className="color-grid">
                <ColorField label="主色调" value={a.accent} onChange={(v) => patchAppearance({ accent: v, selected_bg: v })} />
                <ColorField label="背景色" value={a.bg} onChange={(v) => patchAppearance({ bg: v, theme: "custom" })} />
                <ColorField label="文字颜色" value={a.text} onChange={(v) => patchAppearance({ text: v, theme: "custom" })} />
                <ColorField label="选中背景" value={a.selected_bg} onChange={(v) => patchAppearance({ selected_bg: v, theme: "custom" })} />
                <ColorField label="选中文字" value={a.selected_text} onChange={(v) => patchAppearance({ selected_text: v, theme: "custom" })} />
                <ColorField label="注释颜色" value={a.comment} onChange={(v) => patchAppearance({ comment: v, theme: "custom" })} />
                <ColorField label="边框颜色" value={a.border} onChange={(v) => patchAppearance({ border: v, theme: "custom" })} />
              </div>
            </div>

            <div className="card">
              <h2>皮肤导入导出</h2>
              <p className="hint">
                皮肤文件保存当前外观（主题、配色、字号、圆角、透明度、序号），
                可备份、换机迁移或分享给他人；导入会立即套用。
              </p>
              <div className="btn-row">
                <button className="btn primary" disabled={busy} onClick={handleExportSkin}>
                  导出皮肤
                </button>
                <button
                  className="btn"
                  disabled={busy}
                  onClick={() => skinFileRef.current?.click()}
                >
                  导入皮肤
                </button>
                <input
                  ref={skinFileRef}
                  type="file"
                  accept=".json,application/json"
                  hidden
                  onChange={(e) => handleImportSkin(e.target.files?.[0])}
                />
              </div>
            </div>
          </section>
        )}

        {page === "dict" && (
          <section className="page">
            <h1>词库管理</h1>
            <p className="page-desc">管理自定义短语与词库数据。</p>

            <div className="stat-row">
              <StatCard label="自定义短语" value={stats?.custom_phrase_count ?? 0} unit="条" />
              <StatCard label="词库数据" value={Math.round(stats?.table_size_mb ?? 0)} unit="MB" />
              <StatCard label="用户词典" value={stats?.user_dict_exists ? "已启用" : "未创建"} unit="" />
            </div>

            <div className="card">
              <h2>导入词库</h2>
              <p className="hint">
                导入自定义短语（每行：词&lt;Tab&gt;拼音），追加到用户词库并自动重建索引。
                想迁移打字学到的词请用下面的「备份用户词库」。
              </p>
              <div className="btn-row">
                <button className="btn primary" disabled={busy} onClick={() => fileRef.current?.click()}>
                  选择文件导入
                </button>
                <input
                  ref={fileRef}
                  type="file"
                  accept=".txt,.yaml,.dict,.tsv"
                  hidden
                  onChange={(e) => handleImport(e.target.files?.[0])}
                />
              </div>
            </div>

            <div className="card">
              <h2>导出自定义短语</h2>
              <p className="hint">把自定义短语导出为文本文件（不含打字学到的词）。</p>
              <div className="btn-row">
                <button className="btn" disabled={busy} onClick={handleExport}>
                  导出自定义短语
                </button>
                <button className="btn" disabled={busy} onClick={handleRedeploy}>
                  重建词库索引
                </button>
                <button className="btn" onClick={() => api.openDataDir()}>
                  打开数据目录
                </button>
              </div>
              {stats?.last_import && <p className="toast-line">{stats.last_import}</p>}
            </div>

            <div className="card">
              <h2>用户词库备份与恢复</h2>
              <p className="hint">
                打字学到的词存在 Rime 用户词库（*.userdb）里。备份会把用户词库与自定义短语
                完整另存到数据目录的 backup\ 下；恢复前会自动把当前词库再另存一份，避免误操作丢词。
              </p>
              <div className="btn-row">
                <button className="btn primary" disabled={busy} onClick={handleBackupDict}>
                  备份用户词库
                </button>
                <button className="btn" disabled={busy} onClick={refreshBackups}>
                  刷新备份列表
                </button>
              </div>
              {backups.length > 0 ? (
                <div className="btn-row" style={{ marginTop: 10 }}>
                  <select
                    className="input"
                    style={{ flex: 1, minWidth: 260 }}
                    value={selectedBackup}
                    onChange={(e) => setSelectedBackup(e.target.value)}
                  >
                    {backups.map((b) => (
                      <option key={b} value={b}>
                        {b.split(/[\\/]/).pop()}
                      </option>
                    ))}
                  </select>
                  <button className="btn" disabled={busy} onClick={handleRestoreDict}>
                    恢复所选备份
                  </button>
                </div>
              ) : (
                <p className="hint" style={{ marginTop: 8 }}>
                  暂无备份。
                </p>
              )}
            </div>

          </section>
        )}

        {page === "about" && (
          <section className="page about">
            <div className="about-logo">灵</div>
            <h1>灵犀输入法</h1>
              <p className="about-ver">版本 {version}</p>
              <p className="about-desc">
                系统级 TSF 拼音输入法，基于 Rime 引擎与增强词库，
                支持全角半角、模糊音、状态栏快捷切换、主题定制与词库备份。
              </p>
            <div className="about-tags">
              <span>无广告</span>
              <span>无遥测</span>
                <span>本地运行</span>
                <span>数据自主</span>
              </div>
            <div className="card">
              <h2>更新</h2>
              <p className="hint">
                  本输入法不联网自检。点下面的按钮用浏览器打开发布页，
                  对照最新版本号（当前 {version}）决定是否升级。
              </p>
              <div className="btn-row">
                <button
                  className="btn"
                  onClick={() =>
                    openUrl("https://github.com/itsypa/lingxi-ime/releases/latest").catch((e) =>
                      toastMsg("打开失败：" + String(e))
                    )
                  }
                >
                  查看最新版本
                </button>
              </div>
            </div>

            <div className="card">
              <h2>诊断与维护</h2>
              <p className="hint">
                输入异常时先看日志：日志记录按键、组合、候选与状态栏事件，
                出问题把它发到 issue 里即可定位。
              </p>
              <div className="btn-row">
                <button className="btn small" onClick={handleOpenDataDir}>
                  打开数据目录
                </button>
                <button className="btn small" onClick={handleOpenLogDir}>
                  打开日志目录
                </button>
                <button className="btn small" onClick={handleClearLogs}>
                  清空日志
                </button>
              </div>
            </div>
            <div className="card">
              <h2>快捷键</h2>
              <table className="keys">
                <tbody>
                  <tr><td>Shift</td><td>组合中上屏拼音；未组合切换中/英（切换键可在输入设置里改）</td></tr>
                  <tr><td>回车</td><td>默认把拼音原样上屏（不选词），行为可在输入设置里改</td></tr>
                  <tr><td>空格</td><td>确认高亮候选词</td></tr>
                  <tr><td>1-9</td><td>选择对应候选</td></tr>
                    <tr><td>Ctrl+.</td><td>切换中英标点</td></tr>
                    <tr><td>状态栏「符号」格</td><td>打开/收起符号面板（蓝底=已打开）</td></tr>
                    <tr><td>符号面板</td><td>六类符号点选直上屏；可拖动，✕ 关闭</td></tr>
                    <tr><td>右键状态栏</td><td>弹出开关菜单（中英/全角/标点/符号/设置）</td></tr>
                    <tr><td>← / →</td><td>拼音内逐字移动（跳过分隔符，到头不循环）</td></tr>
                  <tr><td>Tab / [ ] / - =</td><td>候选翻页</td></tr>
                </tbody>
              </table>
            </div>
          </section>
        )}
      </main>

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function NavItem(props: {
  active: boolean;
  icon: string;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      className={`nav-item ${props.active ? "active" : ""}`}
      onClick={props.onClick}
    >
      <span className="nav-icon">{props.icon}</span>
      {props.label}
    </button>
  );
}

function Row(props: {
  label: string;
  desc: string;
  control: React.ReactNode;
}) {
  return (
    <div className="row">
      <div className="row-text">
        <div className="row-label">{props.label}</div>
        <div className="row-desc">{props.desc}</div>
      </div>
      <div className="row-control">{props.control}</div>
    </div>
  );
}

function Toggle(props: {
  checked: boolean;
  labels: [string, string];
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="toggle-wrap">
      <span className={`toggle-label ${!props.checked ? "on" : ""}`}>
        {props.labels[0]}
      </span>
      <button
        className={`toggle ${props.checked ? "on" : ""}`}
        onClick={() => props.onChange(!props.checked)}
      >
        <span className="toggle-knob" />
      </button>
      <span className={`toggle-label ${props.checked ? "on" : ""}`}>
        {props.labels[1]}
      </span>
    </div>
  );
}

function Slider(props: {
  min: number;
  max: number;
  value: number;
  suffix?: string;
  onChange: (v: number) => void;
}) {
  return (
    <div className="slider-wrap">
      <input
        type="range"
        className="slider"
        min={props.min}
        max={props.max}
        value={props.value}
        onChange={(e) => props.onChange(Number(e.target.value))}
      />
      <span className="slider-value">
        {props.value}
        {props.suffix ?? ""}
      </span>
    </div>
  );
}

function ColorField(props: {
  label: string;
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <label className="color-field">
      <input
        type="color"
        value={props.value}
        onChange={(e) => props.onChange(e.target.value)}
      />
      <span>{props.label}</span>
    </label>
  );
}

function StatCard(props: { label: string; value: number | string; unit: string }) {
  return (
    <div className="stat-card">
      <div className="stat-value">
        {props.value}
        {props.unit && <span className="stat-unit">{props.unit}</span>}
      </div>
      <div className="stat-label">{props.label}</div>
    </div>
  );
}

function CandidatePreview({ config }: { config: LingxiConfig }) {
  const a = config.appearance;
  // 固定演示字号：字号设置改由滑块悬停浮层演示，这里不再随字号缩放，
  // 否则预览块会变大/被裁切，把下面的设置挤下去。
  const size = PREVIEW_BASE_SIZE;
  return <PreviewWindow appearance={a} size={size} />;
}

/** pt → 屏幕像素（候选窗按 96 DPI 绘制，1pt = 4/3 px）。 */
function ptToPx(pt: number) {
  return Math.round((pt * 4) / 3);
}

/** 预览块固定演示字号（约 13pt，不随设置变化）。 */
const PREVIEW_BASE_SIZE = ptToPx(13);

function PreviewWindow({
  appearance: a,
  size,
  compact,
}: {
  appearance: AppearanceConfig;
  size: number;
  compact?: boolean;
}) {
  return (
    <div
      className={`preview-window${compact ? " compact" : ""}`}
      style={{
        background: a.bg,
        borderColor: a.border,
        opacity: a.opacity / 100,
        borderRadius: a.corner_radius,
      }}
    >
      {(compact ? PREVIEW_ITEMS.slice(0, 3) : PREVIEW_ITEMS).map((item, i) => {
        const sel = i === 0;
        return (
          <div
            key={item}
            className={`preview-row ${sel ? "sel" : ""}`}
            style={
              sel
                ? { background: a.selected_bg, color: a.selected_text }
                : { color: a.text }
            }
          >
            {a.show_index && (
              <span
                className="preview-index"
                style={{ color: sel ? a.selected_text : a.comment }}
              >
                {i + 1}.
              </span>
            )}
            <span style={{ fontSize: size }}>{item}</span>
          </div>
        );
      })}
    </div>
  );
}

/**
 * 字体大小滑块：滑块本体尺寸固定；鼠标停上去时在旁边弹出浮层，
 * 用当前主题按**该位置对应的字号**渲染候选窗演示（浮层不参与布局，不会挤页面）。
 */
function FontSizeSlider(props: {
  min: number;
  max: number;
  value: number;
  config: LingxiConfig;
  onChange: (v: number) => void;
}) {
  const [hover, setHover] = useState<number | null>(null);
  const [editing, setEditing] = useState(false);
  const ref = useRef<HTMLInputElement>(null);
  // 拖动/键盘调整期间一直显示浮层（此时演示"当前值"），单纯悬停则演示指针处的字号
  useEffect(() => {
    if (!editing) return;
    const stop = () => setEditing(false);
    window.addEventListener("mouseup", stop);
    window.addEventListener("touchend", stop);
    return () => {
      window.removeEventListener("mouseup", stop);
      window.removeEventListener("touchend", stop);
    };
  }, [editing]);
  const sizeOf = (clientX?: number) => {
    const el = ref.current;
    if (!el || clientX === undefined) return props.value;
    const r = el.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (clientX - r.left) / Math.max(1, r.width)));
    return Math.round(props.min + ratio * (props.max - props.min));
  };
  const shown = editing ? props.value : hover;
  return (
    <div
      className="fontsize-ctl"
      onMouseEnter={(e) => setHover(sizeOf(e.clientX))}
      onMouseMove={(e) => setHover(sizeOf(e.clientX))}
      onMouseLeave={() => setHover(null)}
    >
      <div className="slider-wrap">
        <input
          ref={ref}
          type="range"
          className="slider"
          min={props.min}
          max={props.max}
          value={props.value}
          onMouseDown={() => setEditing(true)}
          onFocus={() => setEditing(true)}
          onBlur={() => setEditing(false)}
          onChange={(e) => props.onChange(Number(e.target.value))}
        />
        <span className="slider-value">{props.value} pt</span>
      </div>
      {shown !== null && (
        <div className="fontsize-pop">
          <div className="fontsize-pop-label">
            {editing ? `当前 ${shown} pt` : `${shown} pt 效果（松开/移开即消失）`}
          </div>
          <PreviewWindow
            appearance={props.config.appearance}
            size={ptToPx(shown)}
            compact
          />
        </div>
      )}
    </div>
  );
}

export default App;
