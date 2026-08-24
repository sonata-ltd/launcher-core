use std::time::{Duration, Instant};

use async_channel::{bounded, Sender};
use async_std::task::{self, JoinHandle};
use futures::{
    future::{self, Either},
    pin_mut, StreamExt,
};

use sonata_launcher_core::{
    bus::{event::CoreEvent, EventBus},
    data::registry::operation::message::{
        event::{OperationEvent, OperationUpdate},
        status::Progress,
        OperationMessage,
    },
};

const TICK: Duration = Duration::from_millis(250);

pub struct Reporter {
    stop: Sender<()>,
    handle: JoinHandle<()>,
}

impl Reporter {
    pub async fn finish(self) {
        let _ = self.stop.send(()).await;
        self.handle.await;
    }
}

pub fn spawn_reporter(bus: &EventBus) -> Reporter {
    let mut rx = bus.subscribe();
    let (stop_tx, stop_rx) = bounded::<()>(1);

    let handle = task::spawn(async move {
        let mut last = Instant::now()
            .checked_sub(TICK)
            .unwrap_or_else(Instant::now);

        loop {
            let stopped = {
                let next = rx.next();
                let stopped = stop_rx.recv();

                pin_mut!(next, stopped);

                match future::select(next, stopped).await {
                    Either::Left((Some(event), _)) => {
                        render(event, &mut last);
                        false
                    }
                    Either::Left((None, _)) => return,
                    Either::Right(_) => true,
                }
            };

            if stopped {
                while let Ok(event) = rx.try_recv() {
                    render(event, &mut last);
                }

                return;
            }
        }
    });

    Reporter {
        stop: stop_tx,
        handle,
    }
}

fn render(event: CoreEvent, last: &mut Instant) {
    let CoreEvent::Operation(OperationMessage { data, .. }) = event else {
        return;
    };

    match data {
        OperationEvent::Start(start) => {
            let stages: Vec<String> = start.stages.iter().map(ToString::to_string).collect();

            eprintln!("started: {}", stages.join(" -> "));
        }

        OperationEvent::Update(OperationUpdate::Progress {
            stage, progress, ..
        }) => match progress {
            Progress::Determinable { current, total } => {
                if current < total && last.elapsed() < TICK {
                    return;
                }

                *last = Instant::now();
                eprintln!("  {stage}: {current}/{total}");
            }
            Progress::Indeterminable => eprintln!("{stage}: started"),
        },

        OperationEvent::Update(OperationUpdate::Completed(res)) => {
            eprintln!(
                "{}: {:?} in {:.1}s",
                res.stage, res.status, res.duration_secs
            );
        }

        OperationEvent::Finish(finish) => {
            eprintln!("finished: {:?}", finish.outcome);
        }
    }
}
