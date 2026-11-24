//! ryuplm's command table (Task 9 fills it).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Here {
    Row,
}

pub static COMMANDS: &[ryu_app::command::Command<Here>] = &[];
