use std::cell::UnsafeCell;
use std::mem::take;
use std::sync::atomic::{self, AtomicBool, AtomicU32};
use std::sync::Arc;

use nucleo_matcher::Config;
use parking_lot::Mutex;
use rayon::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use rayon::ThreadPool;

use crate::par_sort::par_quicksort;
use crate::pattern::{self, MultiPattern};
use crate::{boxcar, Match};

struct Matchers(Box<[UnsafeCell<nucleo_matcher::Matcher>]>);

impl Matchers {
    // this is not a true mut from ref, we use a cell here
    #[allow(clippy::mut_from_ref)]
    unsafe fn get(&self) -> &mut nucleo_matcher::Matcher {
        &mut *self.0[rayon::current_thread_index().unwrap()].get()
    }
}

unsafe impl Sync for Matchers {}
unsafe impl Send for Matchers {}

pub(crate) struct Worker<T: Sync + Send + 'static> {
    pub(crate) running: bool,
    matchers: Matchers,
    pub(crate) matches: Vec<Match>,
    pub(crate) pattern: MultiPattern,
    pub(crate) canceled: Arc<AtomicBool>,
    pub(crate) should_notify: Arc<AtomicBool>,
    pub(crate) was_canceled: bool,
    pub(crate) last_snapshot: u32,
    notify: Arc<(dyn Fn() + Sync + Send)>,
    pub(crate) items: Arc<boxcar::Vec<T>>,
    in_flight: Vec<u32>,
}

impl<T: Sync + Send + 'static> Worker<T> {
    pub(crate) fn item_count(&self) -> u32 {
        self.last_snapshot - self.in_flight.len() as u32
    }
    pub(crate) fn update_config(&mut self, config: Config) {
        for matcher in self.matchers.0.iter_mut() {
            matcher.get_mut().config = config.clone();
        }
    }

    // wasm32-wasip1 can't spawn OS threads at all, so a real (>=1 spawned worker)
    // rayon::ThreadPool always fails to build here. `Matchers::get` (below) needs
    // `rayon::current_thread_index()` to return `Some(_)` regardless, so instead of a real
    // pool this registers the *calling* thread itself as a rayon worker, using rayon's own
    // documented single-thread fallback (`ThreadPoolBuilder::use_current_thread`, the same
    // trick rayon-core's own default global registry falls back to when thread spawning is
    // unsupported). That registration is process-wide and permanent (rayon intentionally
    // leaks it for exactly this use case - the `ThreadPool` handle can be dropped right after
    // without losing it), so it only needs to happen once: a second attempt on a thread
    // that's already registered errors out (`CurrentThreadAlreadyInPool`), which every
    // `Worker::new` call after the first one (opening a second picker, say) would hit if this
    // ran unconditionally. There's consequently no per-instance `ThreadPool` to return or to
    // spawn work onto - `Nucleo::tick_inner` runs matching inline on wasm32 instead (see
    // lib.rs).
    #[cfg(target_arch = "wasm32")]
    fn ensure_current_thread_is_rayon_worker() {
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .use_current_thread()
                .build()
                .expect("registering the current thread as a rayon worker failed");
        });
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn new(
        _worker_threads: Option<usize>,
        config: Config,
        notify: Arc<(dyn Fn() + Sync + Send)>,
        cols: u32,
    ) -> Self {
        Self::ensure_current_thread_is_rayon_worker();
        let matchers = (0..1)
            .map(|_| UnsafeCell::new(nucleo_matcher::Matcher::new(config.clone())))
            .collect();
        Worker {
            running: false,
            matchers: Matchers(matchers),
            last_snapshot: 0,
            matches: Vec::new(),
            // just a placeholder
            pattern: MultiPattern::new(cols as usize),
            canceled: Arc::new(AtomicBool::new(false)),
            should_notify: Arc::new(AtomicBool::new(false)),
            was_canceled: false,
            notify,
            items: Arc::new(boxcar::Vec::with_capacity(2 * 1024, cols)),
            in_flight: Vec::with_capacity(64),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn new(
        worker_threads: Option<usize>,
        config: Config,
        notify: Arc<(dyn Fn() + Sync + Send)>,
        cols: u32,
    ) -> (ThreadPool, Self) {
        let worker_threads = worker_threads
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |it| it.get()));
        let pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("nucleo worker {i}"))
            .num_threads(worker_threads)
            .build()
            .expect("creating threadpool failed");
        let matchers = (0..worker_threads)
            .map(|_| UnsafeCell::new(nucleo_matcher::Matcher::new(config.clone())))
            .collect();
        let worker = Worker {
            running: false,
            matchers: Matchers(matchers),
            last_snapshot: 0,
            matches: Vec::new(),
            // just a placeholder
            pattern: MultiPattern::new(cols as usize),
            canceled: Arc::new(AtomicBool::new(false)),
            should_notify: Arc::new(AtomicBool::new(false)),
            was_canceled: false,
            notify,
            items: Arc::new(boxcar::Vec::with_capacity(2 * 1024, cols)),
            in_flight: Vec::with_capacity(64),
        };
        (pool, worker)
    }

    unsafe fn process_new_items(&mut self, unmatched: &AtomicU32) {
        let matchers = &self.matchers;
        let pattern = &self.pattern;
        self.matches.reserve(self.in_flight.len());
        self.in_flight.retain(|&idx| {
            let Some(item) = self.items.get(idx) else {
                return true;
            };
            if let Some(score) = pattern.score(item.matcher_columns, matchers.get()) {
                self.matches.push(Match { score, idx });
            };
            false
        });
        let new_snapshot = self.items.par_snapshot(self.last_snapshot);
        if new_snapshot.end() != self.last_snapshot {
            let end = new_snapshot.end();
            let in_flight = Mutex::new(&mut self.in_flight);
            let items = new_snapshot.map(|(idx, item)| {
                let Some(item) = item else {
                    in_flight.lock().push(idx);
                    unmatched.fetch_add(1, atomic::Ordering::Relaxed);
                    return Match {
                        score: 0,
                        idx: u32::MAX,
                    };
                };
                if self.canceled.load(atomic::Ordering::Relaxed) {
                    return Match { score: 0, idx };
                }
                let Some(score) = pattern.score(item.matcher_columns, matchers.get()) else {
                    unmatched.fetch_add(1, atomic::Ordering::Relaxed);
                    return Match {
                        score: 0,
                        idx: u32::MAX,
                    };
                };
                Match { score, idx }
            });
            self.matches.par_extend(items);
            self.last_snapshot = end;
        }
    }

    fn remove_in_flight_matches(&mut self) {
        let mut off = 0;
        self.in_flight.retain(|&i| {
            let is_in_flight = self.items.get(i).is_none();
            if is_in_flight {
                self.matches.remove((i - off) as usize);
                off += 1;
            }
            is_in_flight
        });
    }

    unsafe fn process_new_items_trivial(&mut self) {
        let new_snapshot = self.items.snapshot(self.last_snapshot);
        if new_snapshot.end() != self.last_snapshot {
            let end = new_snapshot.end();
            let items = new_snapshot.filter_map(|(idx, item)| {
                if item.is_none() {
                    self.in_flight.push(idx);
                    return None;
                };
                Some(Match { score: 0, idx })
            });
            self.matches.extend(items);
            self.last_snapshot = end;
        }
    }

    pub(crate) unsafe fn run(&mut self, pattern_status: pattern::Status, cleared: bool) {
        self.running = true;
        self.was_canceled = false;

        if cleared {
            self.last_snapshot = 0;
            self.in_flight.clear();
            self.matches.clear();
        }

        // TODO: be smarter around reusing past results for rescoring
        if self.pattern.is_empty() {
            self.reset_matches();
            self.process_new_items_trivial();
            if self.should_notify.load(atomic::Ordering::Relaxed) {
                (self.notify)();
            }
            return;
        }

        if pattern_status == pattern::Status::Rescore {
            self.reset_matches();
        }

        let mut unmatched = AtomicU32::new(0);
        if pattern_status != pattern::Status::Unchanged && !self.matches.is_empty() {
            self.process_new_items_trivial();
            let matchers = &self.matchers;
            let pattern = &self.pattern;
            self.matches
                .par_iter_mut()
                .take_any_while(|_| !self.canceled.load(atomic::Ordering::Relaxed))
                .for_each(|match_| {
                    if match_.idx == u32::MAX {
                        debug_assert_eq!(match_.score, 0);
                        unmatched.fetch_add(1, atomic::Ordering::Relaxed);
                        return;
                    }
                    // safety: in-flight items are never added to the matches
                    let item = self.items.get_unchecked(match_.idx);
                    if let Some(score) = pattern.score(item.matcher_columns, matchers.get()) {
                        match_.score = score;
                    } else {
                        unmatched.fetch_add(1, atomic::Ordering::Relaxed);
                        match_.score = 0;
                        match_.idx = u32::MAX;
                    }
                });
        } else {
            self.process_new_items(&unmatched);
        }

        let canceled = par_quicksort(
            &mut self.matches,
            |match1, match2| {
                if match1.score != match2.score {
                    return match1.score > match2.score;
                }
                if match1.idx == u32::MAX {
                    return false;
                }
                if match2.idx == u32::MAX {
                    return true;
                }
                // the tie breaker is comparatively rarely needed so we keep it
                // in a branch especially because we need to access the items
                // array here which involves some pointer chasing
                let item1 = self.items.get_unchecked(match1.idx);
                let item2 = &self.items.get_unchecked(match2.idx);
                let len1: u32 = item1
                    .matcher_columns
                    .iter()
                    .map(|haystack| haystack.len() as u32)
                    .sum();
                let len2 = item2
                    .matcher_columns
                    .iter()
                    .map(|haystack| haystack.len() as u32)
                    .sum();
                if len1 == len2 {
                    match1.idx < match2.idx
                } else {
                    len1 < len2
                }
            },
            &self.canceled,
        );

        if canceled {
            self.was_canceled = true;
        } else {
            self.matches
                .truncate(self.matches.len() - take(unmatched.get_mut()) as usize);
            if self.should_notify.load(atomic::Ordering::Relaxed) {
                (self.notify)();
            }
        }
    }

    fn reset_matches(&mut self) {
        self.matches.clear();
        self.matches
            .extend((0..self.last_snapshot).map(|idx| Match { score: 0, idx }));
        // there are usually only very few in flight items (one for each writer)
        self.remove_in_flight_matches();
    }
}
