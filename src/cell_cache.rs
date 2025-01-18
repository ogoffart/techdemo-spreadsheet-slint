use crate::app::CellContent;
use crate::debouncer::Debouncer;
use core::ops::ControlFlow;
use ehttp::Request;
use ewebsock::{WsEvent, WsMessage, WsSender};
use log::{debug, error, trace, warn};
use lru::LruCache;
use serde_json::json;
use slint::{Model, SharedString};
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

/// The cell as it comes from the backend.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
pub(crate) struct Cell {
    pub(crate) id: u64,
    pub(crate) raw_value: SharedString,
    pub(crate) computed_value: SharedString,
    pub(crate) background: i32,
}

/// A request to update a cell.
#[derive(Debug, Clone, Eq, PartialEq, serde::Serialize)]
pub(crate) struct UpdateCellRequest {
    pub(crate) id: u64,
    pub(crate) raw_value: SharedString,
    pub(crate) background: i32,
}

impl From<&CellContent> for UpdateCellRequest {
    fn from(cell: &CellContent) -> Self {
        Self {
            id: cell.id_low as u64 | (cell.id_high as u64) << 32,
            raw_value: cell.raw_value.clone(),
            background: cell.background_color(),
        }
    }
}

/// We convert Cells from the backend into CellContent that we can edit.
impl From<Cell> for CellContent {
    fn from(cell: Cell) -> Self {
        let [red, blue, green, alpha] = cell.background.to_be_bytes();
        let background = slint::Color::from_argb_u8(alpha, red, green, blue);
        Self {
            id_low: cell.id as u32 as i32,
            id_high: (cell.id >> 32) as u32 as i32,
            background,
            computed_value: cell.computed_value.clone(),
            raw_value: cell.raw_value.clone(),
            //debounce_bg_change: Rc::new(Mutex::new(Debouncer::new())),
        }
    }
}

impl CellContent {
    /// A new empty cell.
    pub(crate) fn empty(id: u64) -> Self {
        Self {
            id_low: id as u32 as i32,
            id_high: (id >> 32) as u32 as i32,
            ..Default::default()
        }
    }

    pub(crate) fn background_color(&self) -> i32 {
        let rgba = self.background.to_argb_u8();
        i32::from_be_bytes([rgba.red, rgba.blue, rgba.green, rgba.alpha])
    }
}

/// Sends a PATCH request to the server to update a cell.
pub fn update_cell(url: String, data: UpdateCellRequest) {
    let request = Request::json(url, &data).unwrap();
    ehttp::fetch(request, move |response| {
        if let Ok(response) = response {
            if !response.ok {
                warn!("POST request failed: {:?}", response.text());
            }
        } else {
            debug!("No response received");
        }
    });
}

pub(crate) struct Loader {
    pub(crate) is_open: AtomicBool,
    ws_sender: Mutex<WsSender>,
}

impl Loader {
    pub(crate) fn new(ws_sender: WsSender) -> Self {
        Self {
            ws_sender: Mutex::new(ws_sender),
            is_open: AtomicBool::new(false),
        }
    }

    pub(crate) fn fetch(&self, range: Range<u64>) -> bool {
        if !self.is_open.load(Ordering::Relaxed) {
            return false;
        }

        let mut sender = self.ws_sender.lock().unwrap();
        sender.send(WsMessage::Text(
            json!({"from": range.start, "to": range.end}).to_string(),
        ));
        true
    }
}

/// The CellCache stores a fixed number of rows in memory.
///
/// - It fetches rows from the backend as needed.
/// - It always contains the cells that the user is currently looking at (and some more
///   since it also prefetches cells around the current view to make scrolling smooth).
/// - It debounces fetching of new rows to avoid fetching too many cells at once.
pub(crate) struct CellCache {
    rows: RefCell<LruCache<usize, Rc<slint::VecModel<CellContent>>>>,
    fetcher: Arc<Loader>,
    debouncer: Rc<RefCell<Debouncer>>,
    current_range: RefCell<Option<Range<u64>>>,
    prefetch_before_after_row: usize,
    max_cells: usize,
    row_size: usize,
    notify: slint::ModelNotify,
}

impl slint::Model for CellCache {
    type Data = slint::ModelRc<CellContent>;

    fn row_count(&self) -> usize {
        self.max_cells / self.row_size
    }

    fn row_data(&self, row: usize) -> Option<Self::Data> {
        let mut rows = self.rows.borrow_mut();
        if let Some(c) = rows.get(&row) {
            Some(slint::ModelRc::from(c.clone()))
        } else {
            let c = Rc::new(self.make_new_row(row));
            rows.push(row, c.clone());
            drop(rows);

            if let Some(current_range) = &*self.current_range.borrow() {
                if current_range.contains(&((row * self.row_size) as u64)) {
                    // Already fetching this range...
                    return Some(slint::ModelRc::from(c));
                }
            }

            let start = row
                .saturating_sub(self.prefetch_before_after_row)
                .saturating_mul(self.row_size);
            let end = std::cmp::min(
                row.saturating_add(self.prefetch_before_after_row)
                    .saturating_mul(self.row_size),
                self.max_cells,
            );
            let current_range = start as u64..end as u64;
            self.current_range.replace(Some(current_range.clone()));
            trace!("fetching range: {:?}", current_range);
            let fetcher = self.fetcher.clone();

            let debouncer_clone = self.debouncer.clone();
            debouncer_clone
                .borrow_mut()
                .debounce(Duration::from_millis(100), move || {
                    let mut max_retry = 10;
                    while !fetcher.fetch(current_range.clone()) && max_retry > 0 {
                        max_retry -= 1;
                    }
                });

            Some(slint::ModelRc::from(c))
        }
    }

    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &self.notify
    }
}

impl CellCache {
    pub(crate) fn new(url: String, width: usize, height: usize) -> Rc<Self> {
        Rc::<Self>::new_cyclic(move |cell_cache| {
            // Change stream connection
            let wrapped = Arc::new(send_wrapper::SendWrapper::new(cell_cache.clone()));
            let ws_sender = ewebsock::ws_connect(
                url.replace("https://", "wss://")
                    .replace("http://", "ws://"),
                Default::default(),
                Box::new(move |e| {
                    let wrapped = wrapped.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(cache) = wrapped.upgrade() {
                            cache.process_event(e);
                        }
                    })
                    .unwrap();
                    ControlFlow::Continue(())
                }),
            )
            .unwrap();

            let lru_cache_size = NonZeroUsize::new(200).unwrap();

            Self {
                rows: LruCache::new(lru_cache_size).into(),
                fetcher: Arc::new(Loader::new(ws_sender)),
                debouncer: Rc::new(RefCell::new(Debouncer::new())),
                current_range: None.into(),
                prefetch_before_after_row: 100,
                max_cells: width * height,
                row_size: width,
                notify: Default::default(),
            }
        })
    }

    fn process_event(&self, event: WsEvent) -> ControlFlow<()> {
        match event {
            WsEvent::Message(WsMessage::Text(update)) => {
                let parsed = serde_json::from_str::<Cell>(&update);
                match parsed {
                    Ok(cell) => {
                        let row = cell.id as usize / self.row_size as usize;
                        let col = cell.id as usize % self.row_size as usize;
                        let mut updated = false;
                        self.rows
                            .borrow_mut()
                            .get_or_insert(row, || {
                                updated = true;
                                Rc::new(self.make_new_row(row))
                            })
                            .set_row_data(col, cell.into());
                        if updated {
                            self.notify.row_changed(row);
                        }
                    }
                    Err(e) => {
                        trace!("error parsing cell update: {:?} {:?}", update, e);
                    }
                }
            }
            WsEvent::Opened => {
                self.fetcher.is_open.store(true, Ordering::Relaxed);
                self.fetcher.fetch(0..2600);
            }
            WsEvent::Closed => {
                self.fetcher.is_open.store(false, Ordering::Relaxed);
            }
            _ => {
                error!("unexpected event: {:?}", event);
            }
        };
        ControlFlow::Continue(())
    }

    fn make_new_row(&self, row_index: usize) -> slint::VecModel<CellContent> {
        let iter =
            (0..self.row_size).map(|i| CellContent::empty((row_index * self.row_size + i) as u64));
        slint::VecModel::from(Vec::from_iter(iter))
    }
}
