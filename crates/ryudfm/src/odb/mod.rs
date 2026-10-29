//! The ODB++ reader: a job, in a folder or an archive, read into a
//! [`ryu_board::model::Board`].

mod error;
pub mod lzw;

pub use error::{Code, Error, Once, fault, location, notice, warning, where_};
