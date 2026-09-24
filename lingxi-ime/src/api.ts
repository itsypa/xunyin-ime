// 设置中心与 Tauri 后端的桥接层。

import { invoke } from "@tauri-apps/api/core";

export interface InputConfig {
  schema: string;
  full_shape: boolean;
  ascii_punct: boolean;
  ascii_mode: boolean;
  simplification: boolean;
  page_size: number;
  beep: boolean;
  /** 自定义中文标点：按键 → 上屏文本（为空即沿用 Rime 内置标点表）。 */
  punct_map: PunctPair[];
}

export interface PunctPair {
  key: string;
  out: string;
}

export interface AppearanceConfig {
  font_size: number;
  theme: string;
  accent: string;
  bg: string;
  text: string;
  selected_bg: string;
  selected_text: string;
  comment: string;
  border: string;
  corner_radius: number;
  opacity: number;
  show_index: boolean;
}

export interface DictConfig {
  custom_phrase_file: string;
}

export interface LingxiConfig {
  input: InputConfig;
  appearance: AppearanceConfig;
  dict: DictConfig;
  version: number;
}

export interface SchemaInfo {
  id: string;
  name: string;
}

export interface DictStats {
  custom_phrase_count: number;
  custom_phrase_file: string;
  table_size_mb: number;
  user_dict_exists: boolean;
  last_import: string;
}

export function initIme(): Promise<void> {
  return invoke("init_ime");
}

export function isMaintenance(): Promise<boolean> {
  return invoke("is_maintenance");
}

export function redeploy(): Promise<void> {
  return invoke("redeploy");
}

export function getConfig(): Promise<LingxiConfig> {
  return invoke("get_config");
}

export function setConfig(cfg: LingxiConfig): Promise<void> {
  return invoke("set_config", { cfg });
}

export function resetConfig(): Promise<LingxiConfig> {
  return invoke("reset_config");
}

export function listSchemas(): Promise<SchemaInfo[]> {
  return invoke("list_schemas");
}

export function getDictStats(): Promise<DictStats> {
  return invoke("get_dict_stats");
}

export function importDictContent(
  filename: string,
  content: string
): Promise<DictStats> {
  return invoke("import_dict_content", { filename, content });
}

export function exportDict(): Promise<string> {
  return invoke("export_dict");
}

export function openDataDir(): Promise<void> {
  return invoke("open_data_dir");
}

/** 备份用户词库（userdb 目录 + 自造词），返回备份目录路径。 */
export function backupUserDict(): Promise<string> {
  return invoke("backup_user_dict");
}

/** 列出已有用户词库备份目录（新的在前）。 */
export function listUserDictBackups(): Promise<string[]> {
  return invoke("list_user_dict_backups");
}

/** 从备份目录恢复用户词库（恢复前会自动另存当前词库）。 */
export function restoreUserDict(dir: string): Promise<string> {
  return invoke("restore_user_dict", { dir });
}

/** 导出当前外观为皮肤文件，返回文件路径。 */
export function exportSkin(): Promise<string> {
  return invoke("export_skin");
}

/** 从皮肤文件内容导入外观设置，返回套用后的完整配置。 */
export function importSkin(content: string): Promise<LingxiConfig> {
  return invoke("import_skin", { content });
}
