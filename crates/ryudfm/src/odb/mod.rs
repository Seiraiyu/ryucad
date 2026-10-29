//! The ODB++ reader: a job, in a folder or an archive, read into a
//! [`ryu_board::model::Board`].

mod error;
pub mod job;
pub mod lzw;
pub mod text;
pub mod units;

pub use error::{Code, Error, Once, fault, location, notice, warning, where_};

pub use job::{Container, Job, Limits};
