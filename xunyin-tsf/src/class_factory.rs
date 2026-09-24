//! COM 类工厂：创建 TextService 实例。

use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows_core::{implement, Interface, Ref, Result, GUID, IUnknown, BOOL};

#[implement(IClassFactory)]
pub struct ClassFactory;

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<'_, IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        crate::tsf_log!("ClassFactory::CreateInstance called");
        unsafe {
            if !punkouter.is_null() {
                return Err(
                    windows::Win32::Foundation::CLASS_E_NOAGGREGATION.into(),
                );
            }
            if ppv.is_null() {
                return Err(windows::Win32::Foundation::E_POINTER.into());
            }
            *ppv = core::ptr::null_mut();
            let service = crate::text_service::TextService::new();
            let tip: windows::Win32::UI::TextServices::ITfTextInputProcessor =
                service.into();
            tip.query(riid, ppv).ok()
        }
    }

    fn LockServer(&self, _flock: BOOL) -> Result<()> {
        Ok(())
    }
}
