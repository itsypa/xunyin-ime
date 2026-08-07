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
