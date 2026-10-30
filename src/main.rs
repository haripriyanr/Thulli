//! # Thulli Desktop Application Entry Point
//!
//! Loads state, builds the shared `AppContext`, creates the window and hands
//! it to each screen's `bind` function. All behaviour lives in `src/app/` and
//! `thulli_core`.

slint::include_modules!();

mod app;

use std::sync::Arc;

use thulli_core::repository::{EncryptedStorageRepository, StorageRepository};
use thulli_core::security::SecurityManager;

use app::AppContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let security = Arc::new(SecurityManager::init()?);
    let repository: Arc<dyn StorageRepository> = Arc::new(EncryptedStorageRepository::new_default(security)?);
    let report = repository.load_with_report();
    let ctx = AppContext::new(repository, report);

    let window = MainWindow::new()?;

    app::shell::bind(&window, &ctx);
    app::today::bind(&window, &ctx);
    app::tasks::bind(&window, &ctx);
    app::notes::bind(&window, &ctx);
    app::insights::bind(&window, &ctx);
    app::lists::bind(&window, &ctx);
    app::shield::bind(&window, &ctx);
    app::assistant::bind(&window, &ctx);
    app::matrix::bind(&window, &ctx);
    app::calendar::bind(&window, &ctx);
    app::reports::bind(&window, &ctx);
    app::focus::bind(&window, &ctx);
    app::preferences::bind(&window, &ctx);

    window.run()?;
    Ok(())
}
