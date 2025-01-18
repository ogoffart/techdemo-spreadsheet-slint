use crate::cell_cache::CellCache;
use crate::http::streaming_request;
use core::ops::ControlFlow;
use log::error;
use serde_json::Deserializer;
use std::rc::Rc;
use std::sync::Arc;

const DEFAULT_COLS: usize = 26;
const DEFAULT_ROWS: usize = 40_000_000; // 26*40_000_000 = 1_040_000_000 cells

pub fn run() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    ui.set_max_cells((DEFAULT_COLS * DEFAULT_ROWS) as i32);

    let server = crate::API_HOST.unwrap_or("http://localhost:3000");

    // Refresh stats
    {
        let ui_weak = std::sync::Mutex::new(ui.as_weak());
        let handle_chunk = Arc::new(move |current_chunk: String| {
            let stream = Deserializer::from_str(&current_chunk).into_iter::<Stats>();
            for maybe_value in stream {
                match maybe_value {
                    Ok(value) => {
                        if let Err(err) = ui_weak
                            .lock()
                            .unwrap()
                            .upgrade_in_event_loop(|ui| ui.set_stats(value))
                        {
                            error!("an error occurred while updating stats: {err}");
                            return ControlFlow::Break(());
                        }
                    }
                    Err(err) => {
                        error!("an error occurred while reading stats: {err}");
                        return ControlFlow::Break(());
                    }
                }
            }
            ControlFlow::Continue(())
        });
        streaming_request(format!("{}/api/stats", server), handle_chunk);
    }

    let url = format!("{}/api/spreadsheet", server);
    let cell_cache = Rc::new(CellCache::new(url.clone(), DEFAULT_COLS, DEFAULT_ROWS));
    ui.set_cells(cell_cache.into());
    let mut col = slint::TableColumn::default();
    col.min_width = 30.;
    col.width = 100.;
    ui.set_columns(slint::ModelRc::new(slint::VecModel::from_iter(
        (0..DEFAULT_COLS).map(|idx| {
            col.title = if idx < 26 {
                slint::format!("{}", (b'A' + idx as u8) as char)
            } else {
                slint::format!(
                    "{}{}",
                    (b'A' + (idx / 26 - 1) as u8) as char,
                    (b'A' + (idx % 26) as u8) as char
                )
            };
            col.clone()
        }),
    )));

    ui.on_update_cell(move |cell| {
        crate::cell_cache::update_cell(url.clone(), (&cell).into());
    });

    ui.on_open_url(move |url| {
        opener::open(url).unwrap();
    });

    ui.show()?;
    slint::run_event_loop()
}

slint::slint! {
    export { MainWindow, Stats } from "ui/main.slint";
}
