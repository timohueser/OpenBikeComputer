use std::ffi::{c_char, c_void, CStr, CString};
use tokenizers::{Tokenizer, TruncationParams};
fn output(value: String) -> *mut c_char {
    CString::new(value).unwrap().into_raw()
}
#[no_mangle]
pub unsafe extern "C" fn obc_tokenizer_create(path: *const c_char, error: *mut *mut c_char) -> *mut c_void {
    let result = (|| {
        let mut tokenizer = Tokenizer::from_file(CStr::from_ptr(path).to_str()?)?;
        tokenizer.with_truncation(Some(TruncationParams { max_length: 64, ..Default::default() }))?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(tokenizer)
    })();
    match result {
        Ok(t) => Box::into_raw(Box::new(t)).cast(),
        Err(e) => {
            *error = output(e.to_string());
            std::ptr::null_mut()
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn obc_tokenizer_encode(
    handle: *mut c_void,
    text: *const c_char,
    length: usize,
    error: *mut *mut c_char,
) -> *mut c_char {
    let result = (|| {
        let t = &*handle.cast::<Tokenizer>();
        let text = std::str::from_utf8(std::slice::from_raw_parts(text.cast(), length))?;
        let e = t.encode_char_offsets(text, true)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
            serde_json::json!({"ids":e.get_ids(),"offsets":e.get_offsets()}).to_string(),
        )
    })();
    match result {
        Ok(s) => output(s),
        Err(e) => {
            *error = output(e.to_string());
            std::ptr::null_mut()
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn obc_tokenizer_destroy(handle: *mut c_void) {
    drop(Box::from_raw(handle.cast::<Tokenizer>()));
}
#[no_mangle]
pub unsafe extern "C" fn obc_tokenizer_string_free(text: *mut c_char) {
    if !text.is_null() {
        drop(CString::from_raw(text));
    }
}
