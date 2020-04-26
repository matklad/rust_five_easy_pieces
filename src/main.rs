extern crate crossbeam_channel;
extern crate rayon;

use crossbeam_channel::{unbounded, Sender};
use std::collections::{hash_map::Entry, HashMap};
use std::sync::{Arc, Mutex};
use std::thread;

fn main() {}

#[test]
fn first() {
    /// The messages sent from the "main" component,
    /// to the other component running in parallel.
    enum WorkMsg {
        Work(u8),
    }

    /// The messages sent from the "parallel" component,
    /// back to the "main component".
    enum ResultMsg {
        Result(u8),
    }

    let (work_sender, work_receiver) = unbounded();
    let (result_sender, result_receiver) = unbounded();

    // Spawn another component in parallel.
    let worker = thread::spawn(move || {
        // Receive, and handle, messages,
        // until told to exit.
        for WorkMsg::Work(num) in work_receiver {
            // Perform some "work", sending back the result.
            result_sender.send(ResultMsg::Result(num)).unwrap();
        }
    });

    // Send two pieces of "work",
    // followed by a request to exit.
    work_sender.send(WorkMsg::Work(0)).unwrap();
    work_sender.send(WorkMsg::Work(1)).unwrap();
    drop(work_sender);

    // A counter of work performed.
    let mut counter = 0;

    for ResultMsg::Result(num) in result_receiver {
        // Assert that we're receiving results
        // in the same order that the requests were sent.
        assert_eq!(num, counter);
        counter += 1;
    }
    // Assert that we're exiting
    // after having received two work results.
    assert_eq!(2, counter);
    // Manual join is also an anti-pattern, better to use something like jod_tread.
    worker.join().unwrap()
}

#[test]
fn second() {
    enum WorkMsg {
        Work(u8),
    }

    enum ResultMsg {
        Result(u8),
    }

    let (work_sender, work_receiver) = unbounded();
    let (result_sender, result_receiver) = unbounded();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();

    let worker = thread::spawn(move || {
        for WorkMsg::Work(num) in work_receiver {
            pool.spawn({
                // Clone the result sender, and move the clone
                // into the spawned worker.
                let result_sender = result_sender.clone();
                move || {
                    // From a worker thread,
                    // do some "work",
                    // and send the result back.
                    result_sender.send(ResultMsg::Result(num)).unwrap();
                }
            });
        }
    });

    work_sender.send(WorkMsg::Work(0)).unwrap();
    work_sender.send(WorkMsg::Work(1)).unwrap();
    drop(work_sender);

    let mut counter = 0;
    for ResultMsg::Result(_) in result_receiver {
        // We cannot make assertions about ordering anymore.
        counter += 1;
    }
    // Well actually, we can now make assertions about result!
    assert_eq!(counter, 2);
    worker.join().unwrap();
    // TODO: check if rayon::ThreadPool joins threads in drop
}

#[test]
fn fourth() {
    enum WorkMsg {
        Work(u8),
    }

    #[derive(Debug, Eq, PartialEq)]
    enum WorkPerformed {
        FromCache,
        New,
    }

    enum ResultMsg {
        Result(u8, WorkPerformed),
    }

    let (work_sender, work_receiver) = unbounded();
    let (result_sender, result_receiver) = unbounded();
    let mut ongoing_work = 0;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();

    // A cache of "work", shared by the workers on the pool.
    let cache: Arc<Mutex<HashMap<u8, u8>>> = Arc::new(Mutex::new(HashMap::new()));

    let worker = thread::spawn(move || {
        for WorkMsg::Work(num) in work_receiver {
            ongoing_work += 1;

            pool.spawn({
                let result_sender = result_sender.clone();
                let cache = cache.clone();
                move || {
                    {
                        // Start of critical section on the cache.
                        let cache = cache.lock().unwrap();
                        if let Some(result) = cache.get(&num) {
                            // We're getting a result from the cache,
                            // send it back,
                            // along with a flag indicating we got it from the cache.
                            result_sender
                                .send(ResultMsg::Result(result.clone(), WorkPerformed::FromCache))
                                .unwrap();
                            return;
                        }
                        // End of critical section on the cache.
                    }

                    // Perform "expensive work" outside of the critical section.
                    // work work work work work work...

                    // Send the result back, indicating we had to perform the work.
                    result_sender
                        .send(ResultMsg::Result(num.clone(), WorkPerformed::New))
                        .unwrap();

                    // Store the result of the "expensive work" in the cache.
                    let mut cache = cache.lock().unwrap();
                    cache.insert(num.clone(), num);
                }
            });
        }
    });

    work_sender.send(WorkMsg::Work(0)).unwrap();
    // Send two requests for the same "work"
    work_sender.send(WorkMsg::Work(1)).unwrap();
    work_sender.send(WorkMsg::Work(1)).unwrap();
    drop(work_sender);

    let mut counter = 0;

    for ResultMsg::Result(_num, _cached) in result_receiver {
        counter += 1;
        // We cannot make assertions about `cached`.
    }
    assert_eq!(3, counter);
    worker.join().unwrap();
}

#[test]
fn fifth() {
    enum WorkMsg {
        Work(u8),
    }

    #[derive(Debug, Eq, PartialEq)]
    enum WorkPerformed {
        FromCache,
        New,
    }

    enum CacheState {
        Ready(u8),
        WorkInProgress(Vec<Sender<ResultMsg>>),
    }

    enum ResultMsg {
        Result(u8, WorkPerformed),
    }

    let (work_sender, work_receiver) = unbounded();
    let (result_sender, result_receiver) = unbounded();
    let mut ongoing_work = 0;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let cache: Arc<Mutex<HashMap<u8, CacheState>>> = Arc::new(Mutex::new(HashMap::new()));

    let worker = thread::spawn(move || {
        for WorkMsg::Work(num) in work_receiver {
            ongoing_work += 1;
            pool.spawn({
                let result_sender = result_sender.clone();
                let cache = cache.clone();

                move || {
                    {
                        let mut cache = cache.lock().unwrap();
                        match cache.entry(num) {
                            Entry::Occupied(mut entry) => {
                                match entry.get_mut() {
                                    CacheState::Ready(result) => {
                                        result_sender
                                            .send(ResultMsg::Result(
                                                result.clone(),
                                                WorkPerformed::FromCache,
                                            ))
                                            .unwrap();
                                    }
                                    CacheState::WorkInProgress(waiters) => {
                                        waiters.push(result_sender);
                                    }
                                }
                                return;
                            }
                            Entry::Vacant(entry) => {
                                entry.insert(CacheState::WorkInProgress(vec![]));
                            }
                        }
                    }

                    let result = num.clone(); // allegedly long-running computation
                    let waiters = {
                        // At this point, we know that we are the first thread, so we should compute the value.
                        let mut cache = cache.lock().unwrap();
                        let waiters = cache.insert(num, CacheState::Ready(result));
                        match waiters {
                            Some(CacheState::WorkInProgress(waiters)) => waiters,
                            _ => unreachable!(),
                        }
                    };
                    result_sender
                        .send(ResultMsg::Result(result, WorkPerformed::New))
                        .unwrap();
                    for waiter in waiters {
                        waiter
                            .send(ResultMsg::Result(result, WorkPerformed::FromCache))
                            .unwrap();
                    }
                }
            });
        }
    });

    work_sender.send(WorkMsg::Work(0)).unwrap();
    work_sender.send(WorkMsg::Work(1)).unwrap();
    work_sender.send(WorkMsg::Work(1)).unwrap();
    drop(work_sender);

    let mut counter = 0;

    // A new counter for work on 1.
    let mut work_one_counter = 0;
    for ResultMsg::Result(num, cached) in result_receiver {
        counter += 1;

        if num == 1 {
            work_one_counter += 1;
        }

        // Now we can assert that by the time
        // the second result for 1 has been received,
        // it came from the cache.
        if num == 1 && work_one_counter == 2 {
            assert_eq!(cached, WorkPerformed::FromCache);
        }
    }
    assert_eq!(3, counter);
    worker.join().unwrap();
}
