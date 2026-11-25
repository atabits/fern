//! Shared SQLite background execution helper and [`DbPath`] GPUI global.

use std::path::{Path, PathBuf};

use anyhow::Result;
use fern_core::Store;
use gpui::{App, AppContext as _, Global, Task};

/// Path to the on-disk `tasks.db` SQLite store, registered as a GPUI global on startup so
/// background tasks can open dedicated WAL connections off the foreground UI thread.
#[derive(Clone, Debug)]
pub struct DbPath(pub PathBuf);

impl Global for DbPath {}

impl DbPath {
    pub fn get(cx: &App) -> Option<PathBuf> {
        cx.try_global::<Self>().map(|p| p.0.clone())
    }
}

/// Runs `op` against an isolated background [`Store`] connection when an on-disk [`DbPath`] global
/// is registered, or falls back to running directly against `fallback_store` (in-memory tests).
pub struct DbRunner;

impl DbRunner {
    /// Spawns a background task opening `db_path` (or falls back synchronously to `fallback_store`
    /// if `db_path` is `None`, e.g. in-memory test suites).
    pub fn run<T: Send + 'static>(
        db_path: Option<PathBuf>,
        fallback_store: &Store,
        cx: &App,
        op: impl FnOnce(&Store) -> Result<T> + Send + 'static,
    ) -> Task<Result<T>> {
        if let Some(path) = db_path {
            cx.background_spawn(async move {
                let store = Store::open(&path)?;
                op(&store)
            })
        } else {
            let result = op(fallback_store);
            Task::ready(result)
        }
    }

    /// Reads [`DbPath`] from `cx` and dispatches [`Self::run`].
    pub fn run_global<T: Send + 'static>(
        fallback_store: &Store,
        cx: &App,
        op: impl FnOnce(&Store) -> Result<T> + Send + 'static,
    ) -> Task<Result<T>> {
        Self::run(DbPath::get(cx), fallback_store, cx, op)
    }

    /// Runs a background task receiving both `Option<&Path>` and an open `&Store`.
    pub fn run_with_path<T: Send + 'static>(
        db_path: Option<PathBuf>,
        fallback_store: &Store,
        cx: &App,
        op: impl FnOnce(Option<&Path>, &Store) -> Result<T> + Send + 'static,
    ) -> Task<Result<T>> {
        if let Some(path) = db_path {
            cx.background_spawn(async move {
                let store = Store::open(&path)?;
                op(Some(path.as_path()), &store)
            })
        } else {
            let result = op(None, fallback_store);
            Task::ready(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn db_runner_runs_background_on_disk_and_falls_back_in_memory(cx: &mut gpui::TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        let dir = std::env::temp_dir().join(format!("fern-dbrunner-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tasks.db");

        let disk_store = Store::open(&path).unwrap();
        disk_store.add_task("Background disk read").unwrap();

        let loaded = Rc::new(RefCell::new(None));
        let loaded_out = Rc::clone(&loaded);
        let titles = cx.update(|cx| {
            cx.set_global(DbPath(path.clone()));
            DbRunner::run_global(&disk_store, cx, |store| {
                Ok(store.tasks()?.into_iter().map(|t| t.title).collect::<Vec<_>>())
            })
        });
        cx.spawn(async move |_| {
            *loaded_out.borrow_mut() = Some(titles.await.unwrap());
        })
        .detach();
        cx.run_until_parked();
        assert_eq!(loaded.borrow().as_deref(), Some(&["Background disk read".to_owned()][..]));

        // Without DbPath global, falls back cleanly to the in-memory store.
        let mem = Store::open_in_memory().unwrap();
        mem.add_task("In-memory fallback").unwrap();
        let mem_loaded = Rc::new(RefCell::new(None));
        let mem_out = Rc::clone(&mem_loaded);
        let mem_titles = cx.update(|cx| {
            DbRunner::run(None, &mem, cx, |store| {
                Ok(store.tasks()?.into_iter().map(|t| t.title).collect::<Vec<_>>())
            })
        });
        cx.spawn(async move |_| {
            *mem_out.borrow_mut() = Some(mem_titles.await.unwrap());
        })
        .detach();
        cx.run_until_parked();
        assert_eq!(
            mem_loaded.borrow().as_deref(),
            Some(&["In-memory fallback".to_owned()][..])
        );

        drop(disk_store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
