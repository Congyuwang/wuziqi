use crate::lobby::messages::{
    CreateAccountFailure, InvalidAccountPassword, LoginFailure, UpdatePasswordFailure,
};
use anyhow::Error;
use bincode::config::Configuration;
use bincode::{config, decode_from_slice, encode_to_vec, Decode, Encode};
use log::error;
use sled::{CompareAndSwapError, Db, Tree};
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const DB_BIN_CONFIG: Configuration = config::standard();
const MAX_USER_NAME_BYTES: usize = 32;
const MIN_PASSWORD_BYTES: usize = 5;
const MAX_PASSWORD_BYTES: usize = 32;
const USER_INFO_TREE: &[u8] = b"user_info";
const META_INFO: &[u8] = b"meta";
const LATEST_USER_ID: &[u8] = b"latest_user_id";

#[derive(Encode, Decode, PartialEq, Eq)]
pub struct Password(pub String);

#[derive(Encode, Decode, PartialEq, Eq)]
pub struct UserInfo {
    pub password: Password,
    pub user_id: u64,
}

impl Deref for Password {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone)]
pub struct LoginValidator {
    db: Db,
    meta: Tree,
    user_info: Tree,
    current_id: Arc<AtomicU64>,
}

impl LoginValidator {
    pub fn init(path: &Path) -> anyhow::Result<Self> {
        let db = sled::open(path).map_err(|_| Error::msg("bad user db path".to_string()))?;
        let user_info = db
            .open_tree(USER_INFO_TREE)
            .map_err(|_| Error::msg("failed to open tree (user info)".to_string()))?;
        let meta = db
            .open_tree(META_INFO)
            .map_err(|_| Error::msg("failed to open tree (meta info)".to_string()))?;
        let zero = encode_to_vec(064, DB_BIN_CONFIG).unwrap();
        let current_id = match meta
            .compare_and_swap(LATEST_USER_ID, None as Option<&[u8]>, Some(zero))
            .map_err(|_| Error::msg("failed to init user db".to_string()))?
        {
            Ok(_) => Arc::new(AtomicU64::new(0)),
            Err(CompareAndSwapError {
                current,
                proposed: _,
            }) => {
                if let Some(current) = current {
                    let (current, _) = decode_from_slice(current.as_ref(), DB_BIN_CONFIG)
                        .map_err(|_| Error::msg("fail to decode u64".to_string()))?;
                    Arc::new(AtomicU64::new(current))
                } else {
                    return Err(Error::msg("unknown error, unreachable code"));
                }
            }
        };
        Ok(Self {
            db,
            meta,
            user_info,
            current_id,
        })
    }

    pub fn query_user_password(&self, name: &str) -> Result<UserInfo, LoginFailure> {
        let name = match validate_name(name) {
            Ok(name) => name,
            Err(e) => return Err(LoginFailure::BadInput(e)),
        };
        match self.user_info.get(name.as_bytes()) {
            Ok(Some(data)) => match decode_from_slice(data.as_ref(), DB_BIN_CONFIG) {
                Ok((info, _)) => Ok(info),
                Err(e) => {
                    error!("user db decode error: {}", e);
                    Err(LoginFailure::ServerError)
                }
            },
            Ok(None) => Err(LoginFailure::AccountDoesNotExist),
            Err(e) => {
                error!("user db query error: {}", e);
                Err(LoginFailure::ServerError)
            }
        }
    }

    /// returns user id if success
    pub fn register_user(
        &self,
        name: &str,
        password: Password,
    ) -> Result<u64, CreateAccountFailure> {
        let name = validate_name(name).map_err(|e| CreateAccountFailure::BadInput(e))?;
        let password =
            validate_password(password).map_err(|e| CreateAccountFailure::BadInput(e))?;

        // generate a new user_id by incrementing a counter
        let new_id = self.current_id.load(Ordering::SeqCst) + 1;
        let id_bytes = match encode_to_vec(new_id, DB_BIN_CONFIG) {
            Ok(id_bytes) => id_bytes,
            Err(_) => {
                error!("u64 encode error");
                return Err(CreateAccountFailure::ServerError);
            }
        };
        if self.meta.insert(LATEST_USER_ID, id_bytes).is_err() {
            error!("meta insertion error");
            return Err(CreateAccountFailure::ServerError);
        } else {
            self.current_id.store(new_id, Ordering::SeqCst);
        }
        let _ = self.meta.flush();

        let new_user_info = UserInfo {
            password,
            user_id: new_id,
        };

        let pass_bytes = encode_to_vec(new_user_info, DB_BIN_CONFIG).map_err(|_| {
            error!("user_info encode error");
            CreateAccountFailure::ServerError
        })?;

        match self
            .user_info
            .compare_and_swap(name, None::<Vec<u8>>, Some(pass_bytes))
        {
            Ok(Ok(())) => Ok(new_id),
            Ok(Err(CompareAndSwapError { current, .. })) => {
                if current.is_some() {
                    Err(CreateAccountFailure::AccountAlreadyExist)
                } else {
                    error!("db swap error at registering user, unknown error");
                    Err(CreateAccountFailure::ServerError)
                }
            }
            Err(e) => {
                error!("db error at registering user ({}): Error {}", name, e);
                Err(CreateAccountFailure::ServerError)
            }
        }
    }

    /// returns user id if success
    pub fn update_user_info(
        &self,
        name: &str,
        old_password: Password,
        new_password: Password,
    ) -> Result<u64, UpdatePasswordFailure> {
        let name = validate_name(name).map_err(|e| UpdatePasswordFailure::BadInput(e))?;
        let old_password =
            validate_password(old_password).map_err(|e| UpdatePasswordFailure::BadInput(e))?;
        let new_password =
            validate_password(new_password).map_err(|e| UpdatePasswordFailure::BadInput(e))?;

        let old_info: UserInfo = match self.user_info.get(name) {
            Ok(info) => match info {
                None => {
                    return Err(UpdatePasswordFailure::UserDoesNotExist);
                }
                Some(info) => {
                    let (info, _) =
                        decode_from_slice(info.as_ref(), DB_BIN_CONFIG).map_err(|_| {
                            error!("password encode error");
                            UpdatePasswordFailure::ServerError
                        })?;
                    info
                }
            },
            Err(_) => {
                error!("password encode error");
                return Err(UpdatePasswordFailure::ServerError);
            }
        };

        if old_password == old_info.password {
            let new_info = UserInfo {
                password: new_password,
                user_id: old_info.user_id,
            };

            let new_info_bytes = encode_to_vec(new_info, DB_BIN_CONFIG).map_err(|_| {
                error!("user info encode error");
                UpdatePasswordFailure::ServerError
            })?;

            match self.user_info.insert(name, new_info_bytes) {
                Ok(_) => Ok(old_info.user_id),
                Err(_) => {
                    error!("user info insertion error");
                    Err(UpdatePasswordFailure::ServerError)
                }
            }
        } else {
            Err(UpdatePasswordFailure::PasswordIncorrect)
        }
    }
}

fn validate_name(name: &str) -> Result<&str, InvalidAccountPassword> {
    if name.len() > MAX_USER_NAME_BYTES {
        Err(InvalidAccountPassword::AccountNameTooLong)
    } else if name.is_empty() {
        Err(InvalidAccountPassword::AccountNameTooShort)
    } else if name.contains("\n") | name.contains("\r") {
        Err(InvalidAccountPassword::BadCharacterAccountName)
    } else {
        Ok(name)
    }
}

/// might return nothing if the message is not login or join
fn validate_password(password: Password) -> Result<Password, InvalidAccountPassword> {
    if password.len() < MIN_PASSWORD_BYTES {
        Err(InvalidAccountPassword::PasswordTooShort)
    } else if password.len() > MAX_PASSWORD_BYTES {
        Err(InvalidAccountPassword::PasswordTooLong)
    } else if password.contains("\n") | password.contains("\r") {
        Err(InvalidAccountPassword::BadCharacterAccountPassword)
    } else {
        Ok(password)
    }
}

impl Drop for LoginValidator {
    fn drop(&mut self) {
        let _ = self.db.flush();
    }
}
