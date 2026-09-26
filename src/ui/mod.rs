//! GTK view layer — widgets and signal wiring only, no business logic
//! (GOAL §5: `ui/` = “仅 GTK 调用与信号连接，不含业务逻辑”).

pub mod dashboard;
pub mod form;
pub mod list;
pub mod toast;
pub mod window;
