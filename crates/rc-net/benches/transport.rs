use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use rc_net::{Cfb8, FrameDecoder, FrameEncoder};

/// Low-entropy data, close to a chunk packet (palettes, repetitive bit arrays).
fn chunk_like(len: usize) -> Vec<u8> {
    let mut x: u32 = 0x1234_5678;
    (0..len)
        .map(|i| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if i % 64 < 48 { (i / 512) as u8 } else { (x >> 24) as u8 & 0x0f }
        })
        .collect()
}

fn cipher(c: &mut Criterion) {
    let mut g = c.benchmark_group("cfb8");
    let mut data = chunk_like(1 << 20);
    g.throughput(Throughput::Bytes(data.len() as u64));
    let mut enc = Cfb8::new(&[3; 16]);
    g.bench_function("encrypt_1MiB", |b| b.iter(|| enc.encrypt(black_box(&mut data))));
    let mut dec = Cfb8::new(&[3; 16]);
    g.bench_function("decrypt_1MiB", |b| b.iter(|| dec.decrypt(black_box(&mut data))));
    g.finish();
    // Split threshold: one core (AES-NI batches) against the whole pool, by buffer size.
    let mut g = c.benchmark_group("cfb8_size");
    for size in [16usize << 10, 64 << 10, 128 << 10, 256 << 10, 1 << 20, 16 << 20] {
        let mut buf = chunk_like(size);
        g.throughput(Throughput::Bytes(size as u64));
        let mut d1 = Cfb8::new(&[3; 16]);
        g.bench_function(format!("batched_{}k", size >> 10), |b| b.iter(|| d1.decrypt_batched(black_box(&mut buf))));
        let mut dn = Cfb8::new(&[3; 16]);
        g.bench_function(format!("parallel_{}k", size >> 10), |b| b.iter(|| dn.decrypt_parallel(black_box(&mut buf))));
    }
    g.finish();
}

fn frames(c: &mut Criterion) {
    let mut g = c.benchmark_group("frames");
    // 64 packets of 40 KiB: order of magnitude of a chunk burst.
    let packets: Vec<Vec<u8>> = (0..64)
        .map(|i| {
            let mut p = chunk_like(40 * 1024);
            p[0] = 0x27;
            p[1] = i as u8;
            p
        })
        .collect();
    let total: usize = packets.iter().map(Vec::len).sum();
    let mut enc = FrameEncoder::new();
    enc.set_compression(256);
    enc.enable_encryption(&[9; 16]);
    let mut wire = Vec::new();
    for p in &packets {
        enc.encode(p, &mut wire).unwrap();
    }
    g.throughput(Throughput::Bytes(total as u64));
    g.bench_function("decode_compressed_encrypted", |b| {
        let mut out = Vec::new();
        b.iter(|| {
            let mut dec = FrameDecoder::new();
            dec.set_compression(256);
            dec.enable_encryption(&[9; 16]);
            for chunk in wire.chunks(64 * 1024) {
                dec.push(chunk);
                while dec.next_packet(&mut out).unwrap() {
                    black_box(&out);
                }
            }
        })
    });
    g.bench_function("encode_compressed_encrypted", |b| {
        let mut out = Vec::new();
        b.iter(|| {
            let mut enc = FrameEncoder::new();
            enc.set_compression(256);
            enc.enable_encryption(&[9; 16]);
            out.clear();
            for p in &packets {
                enc.encode(p, &mut out).unwrap();
            }
            black_box(&out);
        })
    });
    g.finish();
}

criterion_group!(benches, cipher, frames);
criterion_main!(benches);
