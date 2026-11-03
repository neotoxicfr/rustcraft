//! Size of the events crossing the server → client channel, and a micro-bench of the slot size.

use std::mem::size_of;
use std::time::Instant;

use rc_proto_v777::entity::{Delta, Move};

use crate::{EntityEvent, Event};

/// Largest inline payload allowed: the sound packet (frequent, kept inline) is 88 bytes.
const MAX_EVENT_BYTES: usize = 96;

#[test]
fn event_stays_small() {
    // Fails as soon as a variant grows past the largest frequent payload: box the rare variant
    // instead of letting every entity move pay for it.
    assert!(size_of::<Event>() <= MAX_EVENT_BYTES, "Event is {} bytes (limit {MAX_EVENT_BYTES})", size_of::<Event>());
}

#[test]
fn frequent_payloads_stay_inline() {
    // The frequent events must fit without a box: these are the sizes the limit was chosen from.
    assert!(size_of::<EntityEvent>() <= 80, "EntityEvent is {} bytes", size_of::<EntityEvent>());
    assert!(size_of::<rc_proto_v777::play::SoundPacket>() <= 88);
    assert!(size_of::<crate::misc::Misc>() <= 48, "Misc is {} bytes", size_of::<crate::misc::Misc>());
}

fn entity_move(id: i32) -> Event {
    Event::Entity(EntityEvent::Move(Move { id, on_ground: true, delta: Some(Delta::Linear([1, 2, 3])), rot: None }))
}

/// The same event padded to the old slot size (256 bytes), or to the size it would have with the
/// particles kept inline (160 bytes): only the slot size differs from the real `Event`.
#[allow(dead_code)]
enum Padded<const N: usize> {
    Event(Event),
    Pad([u8; N]),
}

/// Nanoseconds per event to push `count` events through an unbounded channel then drain it, on
/// one thread (`threads == 1`) or from a producer thread to this one, like server → main thread.
fn channel_ns_per_event<T: Send + 'static>(count: usize, threads: usize, make: fn(i32) -> T) -> f64 {
    let (tx, rx) = crossbeam_channel::unbounded::<T>();
    let start = Instant::now();
    if threads == 1 {
        for i in 0..count {
            tx.send(make(i as i32)).unwrap();
        }
        drop(tx);
        let received = rx.iter().count();
        assert_eq!(received, count);
    } else {
        let producer = std::thread::spawn(move || {
            for i in 0..count {
                tx.send(make(i as i32)).unwrap();
            }
        });
        let received = rx.iter().count();
        producer.join().unwrap();
        assert_eq!(received, count);
    }
    start.elapsed().as_nanos() as f64 / count as f64
}

/// Micro-bench (ignored by default): `cargo test --release -p rc-client --lib channel_slot_size -- --ignored --nocapture`.
#[test]
#[ignore]
fn channel_slot_size_bench() {
    const COUNT: usize = 2_000_000;
    println!("Event = {} bytes", size_of::<Event>());
    for threads in [1, 2] {
        for pass in 0..3 {
            let now = channel_ns_per_event(COUNT, threads, entity_move);
            let inline_particles = channel_ns_per_event(COUNT, threads, |i| Padded::<152>::Event(entity_move(i)));
            let old = channel_ns_per_event(COUNT, threads, |i| Padded::<248>::Event(entity_move(i)));
            println!(
                "threads {threads} pass {pass}: slot {} B {now:.1} ns, slot {} B {inline_particles:.1} ns, slot {} B {old:.1} ns",
                size_of::<Event>(),
                size_of::<Padded<152>>(),
                size_of::<Padded<248>>()
            );
        }
    }
    // What boxing the particles costs per particle event: one allocation and one free.
    let particle = rc_proto_v777::entity::LevelParticles {
        particle: rc_proto_v777::entity::Particle::plain(1),
        force: false,
        always_visible: false,
        pos: [0.5; 3],
        delta: [0.0; 3],
        speed: [0.0; 3],
        count: 1,
        mode: 0,
    };
    let start = Instant::now();
    for i in 0..COUNT {
        let event = rc_proto_v777::entity::LevelParticles { count: i as i32, ..particle.clone() };
        drop(std::hint::black_box(Box::new(event)));
    }
    println!("box + free of a particle event: {:.1} ns", start.elapsed().as_nanos() as f64 / COUNT as f64);
}
