//! Decoding of the real chunk packets of the reference capture (outside the repository).

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;
use rc_proto::Reader;
use rc_proto_v777::chunk::{ChunkContext, read_chunk};

fn chunk_packets() -> Option<(Vec<Vec<u8>>, usize)> {
    let (cap, d) = (rc_testdata::reference_capture()?, rc_testdata::version_data()?);
    Some((rc_testdata::play_packets(&cap, &d, "minecraft:level_chunk_with_light"), d.states.len()))
}

fn decode(c: &mut Criterion) {
    let Some((packets, states)) = chunk_packets() else {
        eprintln!("capture or cache absent");
        return;
    };
    let ctx = ChunkContext::new(24, states as u32, 67);
    let bytes: usize = packets.iter().map(Vec::len).sum();
    let mut g = c.benchmark_group("chunks");
    g.throughput(Throughput::Bytes(bytes as u64));
    g.bench_function(format!("decode_{}_capture", packets.len()), |b| {
        b.iter(|| {
            for p in &packets {
                black_box(read_chunk(&mut Reader::new(p), &ctx).unwrap());
            }
        })
    });
    // Independent chunks: spread over the pool, measuring scaling.
    use rayon::prelude::*;
    // Machine-dependent steps: powers of 2 up to the number of logical cores.
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut steps: Vec<usize> = std::iter::successors(Some(2usize), |t| Some(t * 2)).take_while(|&t| t < max).collect();
    steps.push(max);
    for threads in steps {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        g.bench_function(format!("decode_parallel_{threads}t"), |b| {
            b.iter(|| {
                pool.install(|| {
                    packets.par_iter().for_each(|p| {
                        black_box(read_chunk(&mut Reader::new(p), &ctx).unwrap());
                    })
                })
            })
        });
    }

    // Real receive chain: decompression then decoding, sequential or spread.
    let mut comp = libdeflater::Compressor::new(libdeflater::CompressionLvl::default());
    let compressed: Vec<(Vec<u8>, usize)> = packets
        .iter()
        .map(|p| {
            let mut out = vec![0u8; comp.zlib_compress_bound(p.len())];
            let n = comp.zlib_compress(p, &mut out).unwrap();
            out.truncate(n);
            (out, p.len())
        })
        .collect();
    let inflate_decode = |(z, len): &(Vec<u8>, usize), dec: &mut libdeflater::Decompressor, buf: &mut Vec<u8>| {
        buf.resize(*len, 0);
        dec.zlib_decompress(z, buf).unwrap();
        black_box(read_chunk(&mut Reader::new(buf), &ctx).unwrap());
    };
    g.bench_function("inflate_decode_1t", |b| {
        let mut dec = libdeflater::Decompressor::new();
        let mut buf = Vec::new();
        b.iter(|| compressed.iter().for_each(|c| inflate_decode(c, &mut dec, &mut buf)))
    });
    g.bench_function(format!("inflate_decode_{max}t"), |b| {
        b.iter(|| {
            compressed.par_iter().for_each_init(
                || (libdeflater::Decompressor::new(), Vec::new()),
                |(dec, buf), c| inflate_decode(c, dec, buf),
            )
        })
    });
    // Client path before reuse: a fresh buffer per packet.
    g.bench_function(format!("inflate_decode_fresh_{max}t"), |b| {
        b.iter(|| {
            compressed
                .par_iter()
                .for_each_init(libdeflater::Decompressor::new, |dec, c| inflate_decode(c, dec, &mut Vec::new()))
        })
    });
    g.finish();
}

criterion_group!(benches, decode);
criterion_main!(benches);
