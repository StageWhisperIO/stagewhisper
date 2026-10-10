use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

pub(crate) fn load(service: &str, account: &str) -> Result<Option<Vec<u8>>, String> {
    match get_generic_password(service, account) {
        Ok(data) => Ok(Some(data.to_vec())),
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        Err(e) => Err(format!("Keychain load failed: {e}")),
    }
}

pub(crate) fn store(service: &str, account: &str, data: &[u8]) -> Result<(), String> {
    set_generic_password(service, account, data).map_err(|e| format!("Keychain store failed: {e}"))
}

pub(crate) fn delete(service: &str, account: &str) -> Result<(), String> {
    match delete_generic_password(service, account) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(e) => Err(format!("Keychain delete failed: {e}")),
    }
}
