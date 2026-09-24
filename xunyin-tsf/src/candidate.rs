//! TSF 候选列表与系统默认候选窗 UI 元素。
//!
//! 实现 ITfCandidateList / ITfCandidateListUIElement / ITfUIElement，
//! 交给 ITfUIElementMgr::BeginUIElement 后由 ctfmon 渲染系统默认候选窗，
//! 无需自绘候选窗口。

use crate::GUID_XUNYIN_UI_ELEMENT;
use std::cell::{Cell, RefCell};
use windows::Win32::UI::TextServices::*;
use windows_core::{implement, Result, BSTR, BOOL, GUID};

/// 单个候选字符串。
#[implement(ITfCandidateString)]
pub struct CandidateString {
    pub text: String,
    pub index: u32,
}

impl ITfCandidateString_Impl for CandidateString_Impl {
    fn GetString(&self) -> Result<BSTR> {
        Ok(BSTR::from(&self.text))
    }

    fn GetIndex(&self) -> Result<u32> {
        Ok(self.index)
    }
}

/// 候选枚举器。
#[implement(IEnumTfCandidates)]
pub struct EnumCandidates {
    pub items: Vec<String>,
    pub pos: Cell<usize>,
}

impl IEnumTfCandidates_Impl for EnumCandidates_Impl {
    fn Clone(&self) -> Result<IEnumTfCandidates> {
        Ok(IEnumTfCandidates::from(EnumCandidates {
            items: self.items.clone(),
            pos: Cell::new(self.pos.get()),
        }))
    }

    fn Next(
        &self,
        ulcount: u32,
        ppcand: *mut Option<ITfCandidateString>,
        pcfetched: *mut u32,
    ) -> Result<()> {
        unsafe {
            let mut fetched = 0u32;
            let mut pos = self.pos.get();
            while fetched < ulcount && pos < self.items.len() {
                if !ppcand.is_null() {
                    *ppcand.add(fetched as usize) = Some(ITfCandidateString::from(
                        CandidateString {
                            text: self.items[pos].clone(),
                            index: pos as u32,
                        },
                    ));
                }
                fetched += 1;
                pos += 1;
            }
            self.pos.set(pos);
            if !pcfetched.is_null() {
                *pcfetched = fetched;
            }
            Ok(())
        }
    }

    fn Reset(&self) -> Result<()> {
        self.pos.set(0);
        Ok(())
    }

    fn Skip(&self, ulcount: u32) -> Result<()> {
        let pos = self.pos.get().min(self.items.len());
        self.pos.set(pos.saturating_add(ulcount as usize).min(self.items.len()));
        Ok(())
    }
}

/// 候选列表 + 系统默认候选窗 UI 元素（单对象多接口）。
#[implement(ITfCandidateList, ITfCandidateListUIElement, ITfUIElement)]
pub struct CandidateListUI {
    candidates: RefCell<Vec<String>>,
    selection: Cell<u32>,
    doc_mgr: RefCell<Option<ITfDocumentMgr>>,
    shown: Cell<bool>,
}

impl CandidateListUI {
    pub fn new() -> Self {
        Self {
            candidates: RefCell::new(Vec::new()),
            selection: Cell::new(0),
            doc_mgr: RefCell::new(None),
            shown: Cell::new(false),
        }
    }

    /// 更新候选数据（由文本服务在每次输入变化后调用）。
    pub fn update(&self, candidates: Vec<String>, selected: usize, doc: Option<ITfDocumentMgr>) {
        *self.candidates.borrow_mut() = candidates;
        self.selection.set(selected as u32);
        *self.doc_mgr.borrow_mut() = doc;
    }
}

impl ITfCandidateList_Impl for CandidateListUI_Impl {
    fn EnumCandidates(&self) -> Result<IEnumTfCandidates> {
        crate::tsf_log!("[cand] EnumCandidates called (count={})", self.candidates.borrow().len());
        Ok(IEnumTfCandidates::from(EnumCandidates {
            items: self.candidates.borrow().clone(),
            pos: Cell::new(0),
        }))
    }

    fn GetCandidate(&self, nindex: u32) -> Result<ITfCandidateString> {
        crate::tsf_log!("[cand] GetCandidate({nindex}) called");
        let items = self.candidates.borrow();
        let text = items
            .get(nindex as usize)
            .cloned()
            .unwrap_or_default();
        Ok(ITfCandidateString::from(CandidateString {
            text,
            index: nindex,
        }))
    }

    fn GetCandidateNum(&self) -> Result<u32> {
        let n = self.candidates.borrow().len() as u32;
        crate::tsf_log!("[cand] GetCandidateNum -> {n}");
        Ok(n)
    }

    fn SetResult(&self, _nindex: u32, _imcr: TfCandidateResult) -> Result<()> {
        // 里程碑 1：键盘选字已由引擎处理；鼠标点击系统候选窗的选择回调
        // 留作后续（需要把选择结果送回引擎并重新注入）。
        Ok(())
    }
}

impl ITfUIElement_Impl for CandidateListUI_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        crate::tsf_log!("[cand] GetDescription called");
        Ok(BSTR::from("迅音输入法候选"))
    }

    fn GetGUID(&self) -> Result<GUID> {
        crate::tsf_log!("[cand] GetGUID called");
        Ok(GUID_XUNYIN_UI_ELEMENT)
    }

    fn Show(&self, bshow: BOOL) -> Result<()> {
        crate::tsf_log!("[cand] Show({}) called", bshow.0);
        self.shown.set(bshow.as_bool());
        Ok(())
    }

    fn IsShown(&self) -> Result<BOOL> {
        let s = self.shown.get();
        crate::tsf_log!("[cand] IsShown -> {s}");
        Ok(BOOL::from(s))
    }
}

impl ITfCandidateListUIElement_Impl for CandidateListUI_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        crate::tsf_log!("[cand] GetUpdatedFlags called");
        Ok(TF_CLUIE_COUNT | TF_CLUIE_SELECTION | TF_CLUIE_STRING | TF_CLUIE_PAGEINDEX)
    }

    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        match self.doc_mgr.borrow().clone() {
            Some(d) => {
                crate::tsf_log!("[cand] GetDocumentMgr -> Some");
                Ok(d)
            }
            None => {
                crate::tsf_log!("[cand] GetDocumentMgr -> None (E_FAIL)");
                Err(windows::Win32::Foundation::E_FAIL.into())
            }
        }
    }

    fn GetCount(&self) -> Result<u32> {
        let n = self.candidates.borrow().len() as u32;
        crate::tsf_log!("[cand] GetCount -> {n}");
        Ok(n)
    }

    fn GetSelection(&self) -> Result<u32> {
        let s = self.selection.get();
        crate::tsf_log!("[cand] GetSelection -> {s}");
        Ok(s)
    }

    fn GetString(&self, uindex: u32) -> Result<BSTR> {
        let text = self
            .candidates
            .borrow()
            .get(uindex as usize)
            .cloned()
            .unwrap_or_default();
        crate::tsf_log!("[cand] GetString({uindex}) -> {text:?}");
        Ok(BSTR::from(text))
    }

    fn GetPageIndex(&self, pindex: *mut u32, usize: u32, pupagecnt: *mut u32) -> Result<()> {
        crate::tsf_log!("[cand] GetPageIndex called (cap={usize})");
        unsafe {
            if usize >= 1 && !pindex.is_null() {
                *pindex = 0;
            }
            if !pupagecnt.is_null() {
                *pupagecnt = 1;
            }
        }
        Ok(())
    }

    fn SetPageIndex(&self, _pindex: *const u32, _upagecnt: u32) -> Result<()> {
        Ok(())
    }

    fn GetCurrentPage(&self) -> Result<u32> {
        crate::tsf_log!("[cand] GetCurrentPage -> 0");
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_core::{ComObject, Interface};

    /// 候选 UI 对象的多接口 vtable：系统渲染候选窗前会 QI 这些接口。
    #[test]
    fn candidate_ui_interfaces_qi() {
        let obj: ComObject<CandidateListUI> = ComObject::new(CandidateListUI::new());
        let ui: ITfUIElement = obj.cast().unwrap();
        let cand_ui: ITfCandidateListUIElement = obj.cast().unwrap();
        let cand_list: ITfCandidateList = obj.cast().unwrap();

        // 各接口都能回到 IUnknown
        let _: windows_core::IUnknown = ui.cast().unwrap();
        let _: windows_core::IUnknown = cand_ui.cast().unwrap();
        let _: windows_core::IUnknown = cand_list.cast().unwrap();

        // 数据读取抽查
        obj.get().update(vec!["你好".to_string(), "泥好".to_string()], 0, None);
        unsafe {
            assert_eq!(cand_list.GetCandidateNum().unwrap(), 2);
            assert_eq!(cand_ui.GetCount().unwrap(), 2);
            assert_eq!(cand_ui.GetString(0).unwrap().to_string(), "你好");
            assert_eq!(ui.GetGUID().unwrap(), GUID_XUNYIN_UI_ELEMENT);
        }
    }
}
