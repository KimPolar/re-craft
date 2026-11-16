use super::*;
use std::cell::RefCell;

fn trace(name: &str) -> Result<String> {
    let center = ChunkPos { x: 0, z: 0 };
    let positions = ticking_dependencies(center);
    let events = RefCell::new(Vec::new());
    let mut preparation = TickingPreparation::start(center, |position| {
        let index = positions
            .iter()
            .position(|&entry| entry == position)
            .unwrap();
        if name == "missing" && index == 1 {
            return None;
        }
        events
            .borrow_mut()
            .push(format!("request:{},{}", position.x, position.z));
        Some(FullFuture {
            id: FullFutureId(index as u64),
            result: None,
        })
    })?;
    let snapshot = |preparation: &TickingPreparation| {
        let state = match preparation.state() {
            TickingFutureState::Waiting => "waiting",
            TickingFutureState::Available => "success",
            TickingFutureState::Unavailable => "error",
            TickingFutureState::Exceptional => "exception",
        };
        format!(
            "{state}/{}/{}",
            usize::from(preparation.has_queued_callback()),
            events.borrow().join(",")
        )
    };
    let initial = snapshot(&preparation);
    events.borrow_mut().clear();
    if name != "missing" {
        for index in 0..8 {
            let result = if index == 2 && name == "failure" {
                FullFutureResult::Unavailable
            } else if index == 2 && name == "exception" {
                FullFutureResult::Exceptional
            } else {
                FullFutureResult::Full
            };
            preparation.complete(FullFutureId(index), result)?;
        }
        if name == "cancel-before" {
            assert!(preparation.cancel());
        }
    }
    let before_last = snapshot(&preparation);
    if name != "missing" {
        preparation.complete(FullFutureId(8), FullFutureResult::Full)?;
    }
    let queued = snapshot(&preparation);
    if name == "cancel-after" {
        assert!(preparation.cancel());
    }
    let outcome = preparation.dispatch(
        |_| {
            events.borrow_mut().push("post".into());
            if name == "throw-post" {
                anyhow::bail!("post failed");
            }
            Ok(())
        },
        |_| {
            events.borrow_mut().push("unpack".into());
            Ok(())
        },
        |_| {
            if name == "send-ready" {
                anyhow::bail!("send notification failed");
            }
            Ok(())
        },
    );
    assert_eq!(
        outcome.is_err(),
        matches!(name, "throw-post" | "send-ready")
    );
    Ok(format!(
        "{name}|{initial}|{before_last}|{queued}|{}",
        snapshot(&preparation)
    ))
}

fn overlap_trace(shared_x: i32, shared_z: i32, reverse: bool) -> Result<String> {
    let mut preparations = TickingPreparations::default();
    let token = |position: ChunkPos| FullFutureId(((position.z + 1) * 4 + position.x + 1) as u64);
    let first = TickingRequestId(1);
    let second = TickingRequestId(2);
    let mut order = [
        (first, ChunkPos { x: 0, z: 0 }),
        (second, ChunkPos { x: 1, z: 0 }),
    ];
    if reverse {
        order.reverse();
    }
    for (id, center) in order {
        preparations.start(id, center, |position| {
            Some(FullFuture {
                id: token(position),
                result: None,
            })
        })?;
    }
    for z in -1..=1 {
        for x in -1..=2 {
            if x == shared_x && z == shared_z {
                continue;
            }
            preparations.complete(token(ChunkPos { x, z }), FullFutureResult::Full)?;
        }
    }
    let before = preparations.queued_len();
    preparations.complete(
        token(ChunkPos {
            x: shared_x,
            z: shared_z,
        }),
        FullFutureResult::Full,
    )?;
    let queued = preparations.queued_len();
    let events = RefCell::new(Vec::new());
    while preparations.queued_len() != 0 {
        preparations.dispatch_next(|request| {
            request.dispatch(
                |position| {
                    events
                        .borrow_mut()
                        .push(format!("post:{},{}", position.x, position.z));
                    Ok(())
                },
                |position| {
                    events
                        .borrow_mut()
                        .push(format!("unpack:{},{}", position.x, position.z));
                    Ok(())
                },
                |_| Ok(()),
            )
        })?;
    }
    Ok(format!(
        "overlap:{shared_x},{shared_z}:{reverse}|{before}|{queued}|{}|{},{}",
        events.borrow().join(","),
        preparations.state(first) == Some(TickingFutureState::Available),
        preparations.state(second) == Some(TickingFutureState::Available)
    ))
}

fn shared_future_trace(name: &str) -> Result<String> {
    let center = ChunkPos { x: 0, z: 0 };
    let positions = ticking_dependencies(center);
    let events = RefCell::new(Vec::new());
    let mut preparation = TickingPreparation::start(center, |position| {
        let index = positions.iter().position(|&pos| pos == position).unwrap();
        events
            .borrow_mut()
            .push(format!("request:{},{}", position.x, position.z));
        Some(FullFuture {
            id: FullFutureId(if index == 7 { 1 } else { index } as u64),
            result: (name == "ready-unavailable" && (index == 1 || index == 7))
                .then_some(FullFutureResult::Unavailable),
        })
    })?;
    let snapshot = |preparation: &TickingPreparation| {
        let state = match preparation.state() {
            TickingFutureState::Waiting => "waiting",
            TickingFutureState::Available => "success",
            TickingFutureState::Unavailable => "error",
            TickingFutureState::Exceptional => "exception",
        };
        format!(
            "{state}/{}/{}",
            usize::from(preparation.has_queued_callback()),
            events.borrow().join(",")
        )
    };
    let initial = snapshot(&preparation);
    events.borrow_mut().clear();
    for index in 0..8 {
        if index != 1 && index != 7 {
            preparation.complete(FullFutureId(index), FullFutureResult::Full)?;
        }
    }
    if name != "ready-unavailable" {
        let result = match name {
            "pending-success" => FullFutureResult::Full,
            "pending-unavailable" => FullFutureResult::Unavailable,
            "pending-exception" => FullFutureResult::Exceptional,
            _ => unreachable!(),
        };
        preparation.complete(FullFutureId(1), result)?;
    }
    let before_last = snapshot(&preparation);
    preparation.complete(FullFutureId(8), FullFutureResult::Full)?;
    let queued = snapshot(&preparation);
    preparation.dispatch(
        |_| {
            events.borrow_mut().push("post".into());
            Ok(())
        },
        |_| {
            events.borrow_mut().push("unpack".into());
            Ok(())
        },
        |_| Ok(()),
    )?;
    Ok(format!(
        "shared:{name}|{initial}|{before_last}|{queued}|{}",
        snapshot(&preparation)
    ))
}

fn shared_overlap_trace(name: &str, reverse: bool) -> Result<String> {
    let token = |position: ChunkPos| {
        if position.z == -1 && matches!(position.x, -1 | 0 | 2) {
            FullFutureId(99)
        } else {
            FullFutureId(((position.z + 1) * 4 + position.x + 1) as u64)
        }
    };
    let mut preparations = TickingPreparations::default();
    let mut order = [
        (TickingRequestId(1), ChunkPos { x: 0, z: 0 }),
        (TickingRequestId(2), ChunkPos { x: 1, z: 0 }),
    ];
    if reverse {
        order.reverse();
    }
    for (id, center) in order {
        preparations.start(id, center, |position| {
            Some(FullFuture {
                id: token(position),
                result: None,
            })
        })?;
    }
    for z in -1..=1 {
        for x in -1..=2 {
            let id = token(ChunkPos { x, z });
            if id != FullFutureId(99) {
                preparations.complete(id, FullFutureResult::Full)?;
            }
        }
    }
    let before = preparations.queued_len();
    let result = match name {
        "success" => FullFutureResult::Full,
        "unavailable" => FullFutureResult::Unavailable,
        "exception" => FullFutureResult::Exceptional,
        _ => unreachable!(),
    };
    preparations.complete(FullFutureId(99), result)?;
    let queued = preparations.queued_len();
    let events = RefCell::new(Vec::new());
    while preparations.queued_len() != 0 {
        preparations.dispatch_next(|request| {
            request.dispatch(
                |position| {
                    events
                        .borrow_mut()
                        .push(format!("post:{},{}", position.x, position.z));
                    Ok(())
                },
                |position| {
                    events
                        .borrow_mut()
                        .push(format!("unpack:{},{}", position.x, position.z));
                    Ok(())
                },
                |_| Ok(()),
            )
        })?;
    }
    let state = |id| match preparations.state(TickingRequestId(id)).unwrap() {
        TickingFutureState::Available => "S",
        TickingFutureState::Unavailable => "U",
        TickingFutureState::Exceptional => "E",
        TickingFutureState::Waiting => "W",
    };
    let output = format!(
        "shared-overlap:{name}:{reverse}|{before}|{queued}|{}|{},{}",
        events.borrow().join(","),
        state(1),
        state(2)
    );
    for id in [TickingRequestId(1), TickingRequestId(2)] {
        assert!(preparations.retire(id)?);
    }
    assert!(preparations.forget_full_future(FullFutureId(99))?);
    Ok(output)
}

fn shuffle(values: &mut [usize], seed: usize) {
    let mut state = seed as u64 + 1;
    for end in (1..values.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        values.swap(end, (state % (end as u64 + 1)) as usize);
    }
}

fn batch_trace(number: usize) -> Result<String> {
    let mut requests = TickingPreparations::default();
    let token = |position: ChunkPos| FullFutureId(((position.z + 1) * 4 + position.x + 1) as u64);
    let mut order: Vec<_> = (0..4).collect();
    shuffle(&mut order, number * 3 + 1);
    for index in order {
        requests.start(
            TickingRequestId(index as u64),
            ChunkPos {
                x: (index % 2) as i32,
                z: (index / 2) as i32,
            },
            |position| {
                Some(FullFuture {
                    id: token(position),
                    result: None,
                })
            },
        )?;
    }
    let mut completions: Vec<_> = (0..16).collect();
    shuffle(&mut completions, number * 3 + 2);
    let mut counts = Vec::new();
    for (index, id) in completions.into_iter().enumerate() {
        let result = if index == number % 16 && number % 3 == 1 {
            FullFutureResult::Unavailable
        } else if index == number % 16 && number % 3 == 2 {
            FullFutureResult::Exceptional
        } else {
            FullFutureResult::Full
        };
        requests.complete(FullFutureId(id as u64), result)?;
        if index == 7 && number % 4 == 0 {
            requests.cancel(TickingRequestId(((number / 4) % 4) as u64));
        }
        counts.push(requests.queued_len().to_string());
    }
    let events = RefCell::new(Vec::new());
    while requests.queued_len() != 0 {
        requests.dispatch_next(|request| {
            request.dispatch(
                |position| {
                    events
                        .borrow_mut()
                        .push(format!("post:{},{}", position.x, position.z));
                    Ok(())
                },
                |position| {
                    events
                        .borrow_mut()
                        .push(format!("unpack:{},{}", position.x, position.z));
                    Ok(())
                },
                |_| Ok(()),
            )
        })?;
    }
    let states = (0..4)
        .map(|index| match requests.state(TickingRequestId(index)) {
            Some(TickingFutureState::Available) => "S",
            Some(TickingFutureState::Unavailable) => "U",
            Some(TickingFutureState::Exceptional) => "E",
            _ => "W",
        })
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        "batch:{number}|{}|{}|{states}",
        counts.join(","),
        events.borrow().join(",")
    ))
}

#[test]
fn ticking_callbacks_wait_for_all_full_futures_and_respect_cancellation() -> Result<()> {
    let success = trace("success")?;
    assert!(success.ends_with("|waiting/0/|waiting/1/|success/0/post,unpack"));
    assert!(trace("missing")?.ends_with("|waiting/1/|waiting/1/|error/0/"));
    assert!(trace("failure")?.ends_with("|waiting/0/|waiting/1/|error/0/"));
    assert!(trace("exception")?.ends_with("|waiting/0/|exception/0/|exception/0/"));
    assert!(trace("cancel-before")?.ends_with("|error/0/|error/0/|error/0/"));
    assert!(trace("cancel-after")?.ends_with("|waiting/0/|waiting/1/|error/0/"));
    assert!(trace("throw-post")?.ends_with("|waiting/0/|waiting/1/|exception/0/post"));
    assert!(trace("send-ready")?.ends_with("|waiting/0/|waiting/1/|exception/0/post,unpack"));
    Ok(())
}

#[test]
fn saved_tick_delay_starts_at_postprocessing_completion() -> Result<()> {
    use crate::worldgen::chunk_ticks::{ChunkTicks, SavedTick, ScheduledTick};
    let center = ChunkPos { x: 0, z: 0 };
    let ticks = RefCell::new(ChunkTicks::from_saved(
        center,
        vec![SavedTick {
            kind: "minecraft:sand".into(),
            position: [1, 4, 1],
            delay: 7,
            priority: 0,
        }],
    )?);
    let positions = ticking_dependencies(center);
    let mut preparation = TickingPreparation::start(center, |position| {
        Some(FullFuture {
            id: FullFutureId(
                positions
                    .iter()
                    .position(|&entry| entry == position)
                    .unwrap() as u64,
            ),
            result: None,
        })
    })?;
    assert_eq!(ticks.borrow().pack(100)[0].delay, 7);
    assert_eq!(ticks.borrow().pack(250)[0].delay, 7);
    for index in 0..9 {
        preparation.complete(FullFutureId(index), FullFutureResult::Full)?;
    }
    assert!(preparation.dispatch(
        |_| {
            assert!(ticks.borrow_mut().schedule(ScheduledTick {
                kind: "minecraft:gravel".into(),
                position: [2, 4, 1],
                trigger: 252,
                priority: 0,
                sub_tick: 0,
            })?);
            Ok(())
        },
        |_| {
            ticks.borrow_mut().unpack(250);
            Ok(())
        },
        |_| Ok(()),
    )?);
    let first = ticks.borrow_mut().poll().unwrap();
    let second = ticks.borrow_mut().poll().unwrap();
    assert_eq!(
        (first.kind.as_str(), first.trigger),
        ("minecraft:gravel", 252)
    );
    assert_eq!(
        (second.kind.as_str(), second.trigger),
        ("minecraft:sand", 257)
    );
    Ok(())
}

#[test]
fn ticking_request_checks_identity_and_wraps_chunk_coordinates() -> Result<()> {
    let center = ChunkPos {
        x: i32::MAX,
        z: i32::MIN,
    };
    let positions = ticking_dependencies(center);
    assert_eq!(
        positions[0],
        ChunkPos {
            x: i32::MAX - 1,
            z: i32::MAX
        }
    );
    assert_eq!(positions[4], center);
    assert_eq!(
        positions[8],
        ChunkPos {
            x: i32::MIN,
            z: i32::MIN + 1
        }
    );
    let mut preparation = TickingPreparation::start(center, |position| {
        Some(FullFuture {
            id: FullFutureId(
                positions
                    .iter()
                    .position(|&entry| entry == position)
                    .unwrap() as u64,
            ),
            result: (position == center).then_some(FullFutureResult::Full),
        })
    })?;
    assert!(!preparation.complete(FullFutureId(4), FullFutureResult::Full)?);
    assert!(
        preparation
            .complete(FullFutureId(10), FullFutureResult::Full)
            .is_err()
    );
    for index in 0..9 {
        if index != 4 {
            preparation.complete(FullFutureId(index), FullFutureResult::Full)?;
        }
    }
    assert!(preparation.has_queued_callback());
    assert!(preparation.cancel());
    assert!(preparation.dispatch(
        |_| panic!("cancelled postprocessing"),
        |_| panic!("cancelled unpack"),
        |_| panic!("cancelled notification")
    )?);
    assert_eq!(preparation.state(), TickingFutureState::Unavailable);
    assert!(!preparation.cancel());
    Ok(())
}

#[test]
fn shared_full_future_updates_all_dependencies_and_rejects_conflicts() -> Result<()> {
    let center = ChunkPos { x: 0, z: 0 };
    let positions = ticking_dependencies(center);
    let mut shared = TickingPreparation::start(center, |_| {
        Some(FullFuture {
            id: FullFutureId(77),
            result: None,
        })
    })?;
    assert_eq!(shared.dependencies().count(), 9);
    assert!(shared.complete(FullFutureId(77), FullFutureResult::Full)?);
    assert!(
        shared
            .dependencies()
            .all(|(_, future)| future.result == Some(FullFutureResult::Full))
    );
    assert!(shared.has_queued_callback());
    shared.dispatch(|_| Ok(()), |_| Ok(()), |_| Ok(()))?;
    assert_eq!(shared.state(), TickingFutureState::Available);

    let mixed = TickingPreparation::start(center, |position| {
        Some(FullFuture {
            id: FullFutureId(11),
            result: (position == positions[4]).then_some(FullFutureResult::Unavailable),
        })
    })?;
    assert!(
        mixed
            .dependencies()
            .all(|(_, future)| future.result == Some(FullFutureResult::Unavailable))
    );
    assert!(mixed.has_queued_callback());
    assert!(
        TickingPreparation::start(center, |position| Some(FullFuture {
            id: FullFutureId(11),
            result: Some(if position == positions[4] {
                FullFutureResult::Full
            } else {
                FullFutureResult::Unavailable
            }),
        }))
        .is_err()
    );
    assert!(
        shared_future_trace("pending-success")?
            .ends_with("|waiting/0/|waiting/1/|success/0/post,unpack")
    );
    assert!(shared_future_trace("ready-unavailable")?.ends_with("|waiting/0/|waiting/1/|error/0/"));
    assert!(
        shared_overlap_trace("success", false)?
            .ends_with("|0|2|post:1,0,unpack:1,0,post:0,0,unpack:0,0|S,S")
    );
    Ok(())
}

#[test]
fn shared_full_futures_dispatch_in_java_dependent_order() -> Result<()> {
    assert_eq!(
        overlap_trace(0, 0, false)?,
        "overlap:0,0:false|0|2|post:1,0,unpack:1,0,post:0,0,unpack:0,0|true,true"
    );
    Ok(())
}

#[test]
fn overlapping_requests_reject_inconsistent_futures_and_retire_cleanly() -> Result<()> {
    let center = ChunkPos { x: 0, z: 0 };
    let positions = ticking_dependencies(center);
    let id_for = |position| {
        FullFutureId(
            positions
                .iter()
                .position(|&entry| entry == position)
                .unwrap() as u64,
        )
    };
    let mut requests = TickingPreparations::default();
    let first = TickingRequestId(1);
    let second = TickingRequestId(2);
    requests.start(first, center, |position| {
        Some(FullFuture {
            id: id_for(position),
            result: None,
        })
    })?;
    assert!(requests.start(first, center, |_| None).is_err());
    assert!(
        requests
            .start(second, center, |position| Some(FullFuture {
                id: id_for(position),
                result: Some(FullFutureResult::Full),
            }))
            .is_err()
    );
    assert!(requests.complete(FullFutureId(0), FullFutureResult::Full)?);
    assert!(!requests.complete(FullFutureId(0), FullFutureResult::Unavailable)?);
    requests.start(second, center, |position| {
        Some(FullFuture {
            id: id_for(position),
            result: None,
        })
    })?;
    assert_eq!(requests.cancel(first), Some(true));
    assert!(requests.retire(first)?);
    assert!(requests.forget_full_future(FullFutureId(0)).is_err());
    for index in 1..9 {
        requests.complete(FullFutureId(index), FullFutureResult::Full)?;
    }
    assert_eq!(requests.queued_len(), 1);
    // An unconsumed callback stays at the front of the executor queue.
    requests.dispatch_next(|_| Ok(()))?;
    assert_eq!(requests.queued_len(), 1);
    assert_eq!(requests.cancel(second), Some(true));
    assert!(requests.retire(second).is_err());
    requests.dispatch_next(|request| {
        request.dispatch(
            |_| panic!("cancelled"),
            |_| panic!("cancelled"),
            |_| panic!("cancelled"),
        )
    })?;
    assert!(requests.retire(second)?);
    for index in 0..9 {
        assert!(requests.forget_full_future(FullFutureId(index))?);
    }
    assert!(!requests.forget_full_future(FullFutureId(0))?);
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn ticking_preparation_matches_java_chunk_map() -> Result<()> {
    let java = crate::worldgen::postprocessing_java_results(
        "TickingChunkFutureOracle",
        include_str!("../tests/java/TickingChunkFutureOracle.java"),
        &[],
    );
    let mut actual = [
        "missing",
        "success",
        "failure",
        "exception",
        "cancel-before",
        "cancel-after",
        "throw-post",
        "send-ready",
    ]
    .into_iter()
    .map(trace)
    .collect::<Result<Vec<_>>>()?;
    for z in -1..=1 {
        for x in 0..=1 {
            for reverse in [false, true] {
                actual.push(overlap_trace(x, z, reverse)?);
            }
        }
    }
    for number in 0..24 {
        actual.push(batch_trace(number)?);
    }
    for name in [
        "ready-unavailable",
        "pending-success",
        "pending-unavailable",
        "pending-exception",
    ] {
        actual.push(shared_future_trace(name)?);
    }
    for name in ["success", "unavailable", "exception"] {
        for reverse in [false, true] {
            actual.push(shared_overlap_trace(name, reverse)?);
        }
    }
    assert_eq!(actual, java);
    Ok(())
}
