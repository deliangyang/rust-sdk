use crate::{config::qiniu_ng_config_t, upload::qiniu_ng_upload_manager_t};
use libc::{c_char, c_void};
use qiniu_ng::Client;
use std::{ffi::CStr, mem::transmute};
use tap::TapOps;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct qiniu_ng_client_t(*mut c_void);

impl From<qiniu_ng_client_t> for Box<Client> {
    fn from(client: qiniu_ng_client_t) -> Self {
        unsafe { Box::from_raw(transmute(client)) }
    }
}

impl From<Box<Client>> for qiniu_ng_client_t {
    fn from(client: Box<Client>) -> Self {
        unsafe { transmute(Box::into_raw(client)) }
    }
}

#[no_mangle]
pub extern "C" fn qiniu_ng_client_new(
    access_key: *const c_char,
    secret_key: *const c_char,
    config: qiniu_ng_config_t,
) -> qiniu_ng_client_t {
    Box::new(Client::new(
        unsafe { CStr::from_ptr(access_key) }.to_str().unwrap().to_owned(),
        unsafe { CStr::from_ptr(secret_key) }.to_str().unwrap().to_owned(),
        config.get_clone(),
    ))
    .into()
}

#[no_mangle]
pub extern "C" fn qiniu_ng_client_free(client: qiniu_ng_client_t) {
    let _ = Box::<Client>::from(client);
}

#[no_mangle]
pub extern "C" fn qiniu_ng_client_get_upload_manager(client: qiniu_ng_client_t) -> qiniu_ng_upload_manager_t {
    let client = Box::<Client>::from(client);
    Box::new(client.upload().to_owned())
        .tap(|_| {
            let _ = qiniu_ng_client_t::from(client);
        })
        .into()
}
