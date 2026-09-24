//! 组合串显示属性：下划线样式，由系统负责渲染。

use crate::GUID_XUNYIN_DISPLAY_ATTR;
use std::cell::Cell;
use windows::Win32::UI::TextServices::*;
use windows_core::{implement, Result, BSTR, BOOL, GUID};

/// 单个显示属性。
#[implement(ITfDisplayAttributeInfo)]
pub struct DisplayAttributeInfo;

impl ITfDisplayAttributeInfo_Impl for DisplayAttributeInfo_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        Ok(GUID_XUNYIN_DISPLAY_ATTR)
    }

    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("迅音输入法组合串"))
    }

    fn GetAttributeInfo(&self, pda: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        unsafe {
            if pda.is_null() {
                return Err(windows::Win32::Foundation::E_POINTER.into());
            }
            *pda = TF_DISPLAYATTRIBUTE {
                crText: TF_DA_COLOR {
                    r#type: TF_CT_NONE,
                    Anonymous: TF_DA_COLOR_0 {
                        cr: windows::Win32::Foundation::COLORREF(0),
                    },
                },
                crBk: TF_DA_COLOR {
                    r#type: TF_CT_NONE,
                    Anonymous: TF_DA_COLOR_0 {
                        cr: windows::Win32::Foundation::COLORREF(0),
                    },
                },
                lsStyle: TF_LS_SOLID,
                fBoldLine: BOOL(0),
                crLine: TF_DA_COLOR {
                    r#type: TF_CT_COLORREF,
                    Anonymous: TF_DA_COLOR_0 {
                        cr: windows::Win32::Foundation::COLORREF(0x00FF_0000),
                    }, // 蓝色下划线 (COLORREF)
                },
                bAttr: TF_ATTR_INPUT,
            };
        }
        Ok(())
    }

    fn SetAttributeInfo(&self, _pda: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        Ok(())
    }

    fn Reset(&self) -> Result<()> {
        Ok(())
    }
}

/// 显示属性枚举器（单元素）。
#[implement(IEnumTfDisplayAttributeInfo)]
pub struct EnumDisplayAttributes {
    done: Cell<bool>,
}

impl EnumDisplayAttributes {
    pub fn new() -> Self {
        Self { done: Cell::new(false) }
    }
}

impl IEnumTfDisplayAttributeInfo_Impl for EnumDisplayAttributes_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(IEnumTfDisplayAttributeInfo::from(EnumDisplayAttributes {
            done: Cell::new(self.done.get()),
        }))
    }

    fn Next(
        &self,
        ulcount: u32,
        rginfo: *mut Option<ITfDisplayAttributeInfo>,
        pcfetched: *mut u32,
    ) -> Result<()> {
        unsafe {
            let mut fetched = 0u32;
            if ulcount >= 1 && !self.done.get() {
                if !rginfo.is_null() {
                    *rginfo = Some(ITfDisplayAttributeInfo::from(DisplayAttributeInfo));
                }
                self.done.set(true);
                fetched = 1;
            }
            if !pcfetched.is_null() {
                *pcfetched = fetched;
            }
            Ok(())
        }
    }

    fn Reset(&self) -> Result<()> {
        self.done.set(false);
        Ok(())
    }

    fn Skip(&self, _ulcount: u32) -> Result<()> {
        self.done.set(true);
        Ok(())
    }
}
