extern crate gfx;
extern crate specs;

use std::{thread, time};
use std::sync::mpsc;

pub type Planner<'a, 'b> = specs::DispatcherBuilder<'a, 'b>;

pub const DRAW_NAME: &'static str = "draw";


pub trait Init<'a, 'b>: 'static {
    type Shell: 'static + Send;
    fn start(self, Planner<'a, 'b>) -> (Self::Shell, Planner<'a, 'b>);
    fn proceed(_: &mut Self::Shell) -> bool { true }
}

struct App<'a, 'b, I: Init<'a, 'b>> {
    shell: I::Shell,
    last_time: time::Instant,
}

impl<'a, 'b, I: Init<'a, 'b>> App<'a, 'b, I> {
    fn tick(&mut self) -> bool {
        let elapsed = self.last_time.elapsed();
        self.last_time = time::Instant::now();
        let delta = elapsed.subsec_nanos() as f32 / 1e9 + elapsed.as_secs() as f32;
        I::proceed(&mut self.shell)
    }
}

struct ChannelPair<R: gfx::Resources, C: gfx::CommandBuffer<R>> {
    receiver: mpsc::Receiver<gfx::Encoder<R, C>>,
    sender: mpsc::Sender<gfx::Encoder<R, C>>,
}

pub trait Painter<'a, R: gfx::Resources>: 'static + Send {
    type SystemData: specs::SystemData<'a>;
    fn draw<C>(&mut self, sys_data: Self::SystemData, &mut gfx::Encoder<R, C>) where
            C: gfx::CommandBuffer<R>;
}

struct DrawSystem<R: gfx::Resources, C: gfx::CommandBuffer<R>, P> {
    painter: P,
    channel: ChannelPair<R, C>,
}

impl<'a, R, C, P> specs::System<'a> for DrawSystem<R, C, P>
where
    R: 'static + gfx::Resources,
    C: 'static + Send + gfx::CommandBuffer<R>,
    P: Painter<'a, R>,
{
    type SystemData = P::SystemData;

    fn run(&mut self, sys_data: Self::SystemData) {
        // get a new command buffer
        let mut encoder = match self.channel.receiver.recv() {
            Ok(r) => r,
            Err(_) => return,
        };
        // render entities
        self.painter.draw(sys_data, &mut encoder);
        // done
        let _ = self.channel.sender.send(encoder);
    }
}

pub struct Pegasus<D: gfx::Device> {
    pub device: D,
    channel: ChannelPair<D::Resources, D::CommandBuffer>,
    _guard: thread::JoinHandle<()>,
}

pub struct Swing<'a, D: 'a + gfx::Device> {
    device: &'a mut D,
}

impl<'a, D: 'a + gfx::Device> Drop for Swing<'a, D> {
    fn drop(&mut self) {
        self.device.cleanup();
    }
}

impl<D: gfx::Device> Pegasus<D> {
    pub fn new<'a, 'b, F, I, P>(init: I, device: D, painter: P, mut com_factory: F)
               -> Pegasus<D> where
        I: Init<'a, 'b>,
        D::CommandBuffer: 'static + Send, //TODO: remove when gfx forces these bounds
        P: for<'c> Painter<'c, D::Resources>,
        F: FnMut() -> D::CommandBuffer,
    {
        let (app_send, dev_recv) = mpsc::channel();
        let (dev_send, app_recv) = mpsc::channel();

        // double-buffering renderers
        for _ in 0..2 {
            let enc = gfx::Encoder::from(com_factory());
            app_send.send(enc).unwrap();
        }

        let mut app = {
            let draw_sys = DrawSystem {
                painter: painter,
                channel: ChannelPair {
                    receiver: app_recv,
                    sender: app_send,
                },
            };
            let w = specs::World::new();
            let mut dispatcher = specs::DispatcherBuilder::new()
                .add(draw_sys, DRAW_NAME, &[]);
            let (shell, dispatcher) = init.start(dispatcher);
            dispatcher.build().dispatch(&mut w.res);

            App::<I> {
                shell: shell,
                last_time: time::Instant::now(),
            }
        };

        Pegasus {
            device: device,
            channel: ChannelPair {
                sender: dev_send,
                receiver: dev_recv,
            },
            _guard: thread::spawn(move || {
                while app.tick() {}
            }),
        }
    }

    pub fn swing(&mut self) -> Option<Swing<D>> {
        match self.channel.receiver.recv() {
            Ok(mut encoder) => {
                // draw a frame
                encoder.flush(&mut self.device);
                if self.channel.sender.send(encoder).is_err() {
                    return None
                }
                Some(Swing {
                    device: &mut self.device,
                })
            },
            Err(_) => None,
        }
    }
}
