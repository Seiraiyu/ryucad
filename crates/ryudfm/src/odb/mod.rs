//! The ODB++ reader: a job, in a folder or an archive, read into a
//! [`ryu_board::model::Board`].

pub mod attrs;
pub mod eda;
mod error;
pub mod features;
pub mod font;
pub mod info;
pub mod job;
pub mod lzw;
pub mod matrix;
pub mod outline;
pub mod step;
mod sysattr;
pub mod text;
pub mod tools;
pub mod units;

pub use error::{Code, Error, Once, fault, location, notice, warning, where_};

pub use job::{Container, Job, Limits};
