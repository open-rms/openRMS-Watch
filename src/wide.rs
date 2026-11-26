//! Wideband scanning: one SDR, whole band segments, a pool of Mercury decoders.
//!
//!   IQ command --> reader --> segment threads (mix + decimate to the segment)
//!                               |-- detector: DATAC16-shaped bursts
//!                               |-- 5 s ring buffer (pre-roll)
//!                               '-- USB extractors --audio--> decoder slots (Mercury)
//!
//! A detection asks the Manager for a decoder slot; the segment thread then
//! replays its ring buffer into that slot and keeps feeding it live. A slot
//! that hears nothing is released after a short hold; one that decodes a
//! Mercury frame is kept longer; one that hears the RMS itself (ACCEPT or CQ)
//! is pinned, so the watcher learns the band's RMS frequencies by itself.

use crate::cdsp::{Chain, Nco, UsbExtractor, AUDIO_CENTER};
use crate::config::{Receiver, Wideband};
use crate::db::{now_ms, Heard};
use crate::detect::Detector;
use crate::runner::{mkfifo, open_fifo_writer, spawn_group, Event, Group};
use anyhow::{Context, Result};
use num_complex::Complex32 as C32;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver as MpscRx, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::{info, warn};

const BLOCK: usize = 1 << 16; // complex samples per IQ block
const UNCONFIRMED_HOLD: Duration = Duration::from_secs(45);
const CONFIRMED_HOLD: Duration = Duration::from_secs(15 * 60);
const COOLDOWN: Duration = Duration::from_secs(120);
const SAME_FREQ_HZ: f64 = 60.0;

// ------------------------------------------------------------------ manager

#[derive(Clone)]
pub struct Assign {
    pub seg: usize,
    pub rf_center: f64,
    last_activity: Instant,
    confirmed: bool,
    pinned: bool,
}

struct Slot {
    assign: Option<Assign>,
    /// Last assignment, so a frame that arrives just after release is still tagged.
    last: Option<Assign>,
}

pub struct Manager {
    slots: Vec<Slot>,
    gens: Arc<Vec<AtomicU64>>,
    cooldown: Vec<(f64, Instant)>,
    /// Frequencies that keep triggering without decoding (interference spurs,
    /// RTTY...): each strike multiplies the cooldown by 4, up to an hour.
    strikes: Vec<(f64, u32)>,
    max_pinned: usize,
}

impl Manager {
    fn new(n: usize) -> Manager {
        Manager {
            slots: (0..n).map(|_| Slot { assign: None, last: None }).collect(),
            gens: Arc::new((0..n).map(|_| AtomicU64::new(0)).collect()),
            cooldown: Vec::new(),
            strikes: Vec::new(),
            max_pinned: (n / 2).max(1),
        }
    }

    fn bump(&self, i: usize) -> u64 {
        let g = self.gens[i].fetch_add(1, Ordering::SeqCst) + 1;
        self.publish();
        g
    }

    /// Mirror the pool into the live status.
    fn publish(&self) {
        let decs = self
            .slots
            .iter()
            .enumerate()
            .map(|(i, s)| match &s.assign {
                Some(a) => crate::live::DecLive {
                    idx: i,
                    state: if a.pinned { "pinned" } else if a.confirmed { "confirmed" } else { "listening" },
                    dial_hz: Some((a.rf_center - AUDIO_CENTER).max(0.0) as u64),
                    since_ms: Some(now_ms() - a.last_activity.elapsed().as_millis() as i64),
                },
                None => crate::live::DecLive { idx: i, state: "idle", dial_hz: None, since_ms: None },
            })
            .collect();
        let mut l = crate::live::get().lock().unwrap();
        // one pool per process today; several widebands append in order
        l.decoders = decs;
    }

    /// A burst at `rf`: returns (slot, generation) if a decoder should attach.
    fn on_detection(&mut self, seg: usize, rf: f64, now: Instant) -> Option<(usize, u64)> {
        for s in self.slots.iter_mut() {
            if let Some(a) = s.assign.as_mut() {
                if (a.rf_center - rf).abs() < SAME_FREQ_HZ {
                    a.last_activity = now;
                    return None;
                }
            }
        }
        self.cooldown.retain(|(_, until)| *until > now);
        if self.cooldown.iter().any(|(f, _)| (f - rf).abs() < SAME_FREQ_HZ) {
            return None;
        }
        let idx = self.slots.iter().position(|s| s.assign.is_none()).or_else(|| {
            // evict: unconfirmed first, then confirmed, oldest activity; never pinned
            self.slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.assign.as_ref().is_some_and(|a| !a.pinned))
                .min_by_key(|(_, s)| {
                    let a = s.assign.as_ref().unwrap();
                    (a.confirmed, a.last_activity)
                })
                .map(|(i, _)| i)
        })?;
        if let Some(old) = self.slots[idx].assign.take() {
            info!("decoder {idx}: evicted {:.0} Hz for {:.0} Hz", old.rf_center, rf);
            self.slots[idx].last = Some(old);
        }
        self.slots[idx].assign = Some(Assign { seg, rf_center: rf, last_activity: now, confirmed: false, pinned: false });
        info!("decoder {idx}: listening at {:.0} Hz (dial {:.0})", rf, rf - AUDIO_CENTER);
        Some((idx, self.bump(idx)))
    }

    /// A Mercury frame decoded on slot `i`. Returns the assignment to tag it with.
    fn on_frame(&mut self, i: usize, kind: &str, now: Instant) -> Option<Assign> {
        let pinned_now = self.slots.iter().filter(|s| s.assign.as_ref().is_some_and(|a| a.pinned)).count();
        let slot = &mut self.slots[i];
        match slot.assign.as_mut() {
            Some(a) => {
                a.confirmed = true;
                a.last_activity = now;
                if matches!(kind, "ACCEPT" | "CQ") && !a.pinned && pinned_now < self.max_pinned {
                    a.pinned = true;
                    info!("decoder {i}: pinned {:.0} Hz (RMS heard)", a.rf_center);
                }
                let a = a.clone();
                self.publish();
                Some(a)
            }
            None => slot.last.clone(),
        }
    }

    fn expire(&mut self, now: Instant) {
        for i in 0..self.slots.len() {
            let release = match &self.slots[i].assign {
                Some(a) if a.pinned => false,
                Some(a) if a.confirmed => now.duration_since(a.last_activity) > CONFIRMED_HOLD,
                Some(a) => now.duration_since(a.last_activity) > UNCONFIRMED_HOLD,
                None => false,
            };
            if release {
                let a = self.slots[i].assign.take().unwrap();
                if !a.confirmed {
                    let n = match self.strikes.iter_mut().find(|(f, _)| (f - a.rf_center).abs() < SAME_FREQ_HZ) {
                        Some(s) => {
                            s.1 += 1;
                            s.1
                        }
                        None => {
                            self.strikes.push((a.rf_center, 1));
                            1
                        }
                    };
                    let hold = (COOLDOWN * 4u32.pow((n - 1).min(4))).min(Duration::from_secs(3600));
                    self.cooldown.push((a.rf_center, now + hold));
                } else {
                    self.strikes.retain(|(f, _)| (f - a.rf_center).abs() >= SAME_FREQ_HZ);
                }
                info!("decoder {i}: released {:.0} Hz{}", a.rf_center, if a.confirmed { "" } else { " (nothing decoded)" });
                self.slots[i].last = Some(a);
                self.bump(i);
            }
        }
    }
}

// ------------------------------------------------------------------ setup

pub struct WideCtx {
    pub cfg: Wideband,
    pub receiver: Receiver,
    /// DB channel id per segment.
    pub seg_channels: Vec<i64>,
    pub dir: PathBuf,
    pub mercury: PathBuf,
    /// One control port per decoder (data = +1, broadcast = +2), already checked free.
    pub ports: Vec<u16>,
    pub connect_only: bool,
}

/// Pick the segment's decimation: the largest divisor of rate/8000 that still
/// leaves 25% margin over the segment width.
fn seg_decim(rate: u64, width: f64) -> usize {
    let k = (rate / 8000) as usize;
    (1..=k)
        .rev()
        .find(|d| k % d == 0 && rate as f64 / *d as f64 >= 1.25 * width && (k / d) >= 1)
        .unwrap_or(1)
}

pub fn start(ctx: WideCtx, tx: Sender<Event>, stop: Arc<AtomicBool>) -> Result<Vec<JoinHandle<()>>> {
    let n = ctx.ports.len();
    let mgr = Arc::new(Mutex::new(Manager::new(n)));
    mgr.lock().unwrap().publish();
    let gens = mgr.lock().unwrap().gens.clone();
    let mut handles = Vec::new();

    // decoder slots
    let mut slot_tx = Vec::new();
    for i in 0..n {
        let (stx, srx) = sync_channel::<Vec<i16>>(512);
        slot_tx.push(stx);
        let slot = SlotCtx {
            idx: i,
            port: ctx.ports[i],
            dir: ctx.dir.join(format!("decoder{i}")),
            mercury: ctx.mercury.clone(),
            connect_only: ctx.connect_only,
            receiver: ctx.receiver.clone(),
            seg_channels: ctx.seg_channels.clone(),
        };
        let (mgr, tx, stop) = (mgr.clone(), tx.clone(), stop.clone());
        let rx = Arc::new(Mutex::new(srx));
        handles.push(std::thread::Builder::new().name(format!("decoder{i}")).spawn(move || slot.run(rx, mgr, tx, stop))?);
    }

    // segment threads
    let rate = ctx.cfg.rate as f64;
    let mut seg_in = Vec::new();
    for (si, [lo, hi]) in ctx.cfg.segments.iter().enumerate() {
        let (itx, irx) = sync_channel::<Arc<Vec<C32>>>(64);
        seg_in.push(itx);
        let width = (*hi - *lo) as f64;
        let center = (*lo + *hi) as f64 / 2.0;
        let d = seg_decim(ctx.cfg.rate, width);
        let seg_rate = rate / d as f64;
        info!(
            "segment {}: {:.3}-{:.3} MHz, {:.0} kS/s",
            si, *lo as f64 / 1e6, *hi as f64 / 1e6, seg_rate / 1e3
        );
        let mut det = Detector::new(seg_rate, -width / 2.0, width / 2.0);
        if let Some(t) = ctx.cfg.threshold_db {
            det.threshold_db = t;
        }
        let seg = SegCtx {
            idx: si,
            nco: Nco::new(-(center - ctx.cfg.center_hz as f64), rate),
            chain: Chain::new(rate, d, width / 2.0 * 1.02, seg_rate / 2.0),
            seg_rate,
            center,
            det,
            ring: VecDeque::new(),
            ring_len: 0,
            ring_max: (ctx.cfg.preroll_s * seg_rate) as usize,
            extractors: Vec::new(),
            channel_id: ctx.seg_channels[si],
            in_rate: rate,
            live_idx: {
                let mut l = crate::live::get().lock().unwrap();
                l.segments.push(crate::live::SegLive {
                    receiver: ctx.receiver.id.clone(), lo_hz: *lo, hi_hz: *hi, ..Default::default()
                });
                l.segments.len() - 1
            },
        };
        let (mgr, gens, slot_tx, tx, stop) = (mgr.clone(), gens.clone(), slot_tx.clone(), tx.clone(), stop.clone());
        handles.push(std::thread::Builder::new().name(format!("seg{si}")).spawn(move || {
            seg.run(irx, mgr, gens, slot_tx, tx, stop)
        })?);
    }

    // IQ reader (supervised)
    {
        let stop = stop.clone();
        handles.push(std::thread::Builder::new().name("iq-reader".into()).spawn(move || {
            let mut backoff = 2;
            while !stop.load(Ordering::Relaxed) {
                match read_iq(&ctx, &seg_in, &stop) {
                    Ok(true) => {
                        info!("wideband {}: file finished", ctx.receiver.id);
                        return;
                    }
                    Ok(false) => {}
                    Err(e) => warn!("wideband {}: {e:#}", ctx.receiver.id),
                }
                for _ in 0..backoff * 10 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                backoff = (backoff * 2).min(60);
            }
        })?);
    }
    Ok(handles)
}

/// Ok(true): a non-looping file ended.
fn read_iq(ctx: &WideCtx, outs: &[SyncSender<Arc<Vec<C32>>>], stop: &AtomicBool) -> Result<bool> {
    if let Some(args) = &ctx.cfg.device {
        return read_soapy(ctx, args, outs, stop);
    }
    let bps = match ctx.cfg.format.as_str() {
        "cs16" => 4,
        _ => 2,
    };
    let (_child, mut src): (Option<Group>, Box<dyn Read>) = if let Some(cmd) = &ctx.cfg.command {
        let cmd = cmd
            .replace("{center_hz}", &ctx.cfg.center_hz.to_string())
            .replace("{rate}", &ctx.cfg.rate.to_string())
            .replace("{ppm}", &ctx.receiver.ppm.to_string());
        std::fs::create_dir_all(&ctx.dir)?;
        let errlog = std::fs::File::create(ctx.dir.join("sdr.log"))?;
        let mut g = spawn_group(
            Command::new("sh").arg("-c").arg(&cmd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(errlog),
        )
        .with_context(|| format!("starting {cmd}"))?;
        info!("wideband {}: {cmd}", ctx.receiver.id);
        let out = g.0.stdout.take().unwrap();
        (Some(g), Box::new(out))
    } else {
        let p = ctx.cfg.file.clone().unwrap();
        // OPENRMS_WATCH_BENCH=1 replays as fast as possible (throughput test)
        let pace = if std::env::var_os("OPENRMS_WATCH_BENCH").is_some() { 1e12 } else { ctx.cfg.rate as f64 * bps as f64 };
        let f = crate::runner::PacedFile::open_rate(&p, ctx.cfg.loop_file, pace)?;
        (None, Box::new(f))
    };
    let mut raw = vec![0u8; BLOCK * bps];
    let mut dropped = 0u64;
    let mut last_report = Instant::now();
    let mut total = 0u64;
    let t_start = Instant::now();
    let mut lvl_t = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let mut got = 0;
        while got < raw.len() {
            match src.read(&mut raw[got..]) {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        let ns = got / bps;
        if ns == 0 {
            if ctx.cfg.file.is_some() {
                info!("wideband {}: read {} samples in {:.1} s ({:.2} MS/s)", ctx.receiver.id, total,
                      t_start.elapsed().as_secs_f64(), total as f64 / t_start.elapsed().as_secs_f64() / 1e6);
            }
            return Ok(ctx.cfg.file.is_some());
        }
        total += ns as u64;
        let mut v = Vec::with_capacity(ns);
        match ctx.cfg.format.as_str() {
            "cs8" => {
                for i in 0..ns {
                    v.push(C32::new(raw[2 * i] as i8 as f32 / 128.0, raw[2 * i + 1] as i8 as f32 / 128.0));
                }
            }
            "cu8" => {
                for i in 0..ns {
                    v.push(C32::new((raw[2 * i] as f32 - 127.5) / 128.0, (raw[2 * i + 1] as f32 - 127.5) / 128.0));
                }
            }
            _ => {
                for i in 0..ns {
                    let re = i16::from_le_bytes([raw[4 * i], raw[4 * i + 1]]) as f32 / 2048.0;
                    let im = i16::from_le_bytes([raw[4 * i + 2], raw[4 * i + 3]]) as f32 / 2048.0;
                    v.push(C32::new(re, im));
                }
            }
        }
        live_levels(&ctx.receiver.id, &v, ctx.cfg.rate as f64, &mut lvl_t);
        let block = Arc::new(v);
        // never block the SDR read: a lagging segment drops blocks
        if std::env::var_os("OPENRMS_WATCH_BENCH").is_some() {
            for o in outs {
                if o.send(block.clone()).is_err() {
                    return Ok(false);
                }
            }
            continue;
        }
        for o in outs {
            match o.try_send(block.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => dropped += 1,
                Err(TrySendError::Disconnected(_)) => return Ok(false),
            }
        }
        if dropped > 0 && last_report.elapsed() > Duration::from_secs(10) {
            warn!("wideband {}: CPU can't keep up, dropped {dropped} blocks", ctx.receiver.id);
            dropped = 0;
            last_report = Instant::now();
        }
    }
    Ok(false)
}

/// Publish SDR levels about twice a second.
fn live_levels(id: &str, block: &[C32], rate: f64, last: &mut Instant) {
    if last.elapsed() < Duration::from_millis(500) {
        return;
    }
    *last = Instant::now();
    let (clip, rms) = crate::live::levels(block);
    let mut l = crate::live::get().lock().unwrap();
    let r = l.receivers.entry(id.to_string()).or_default();
    r.clip_pct = (clip * 1000.0).round() / 1000.0;
    r.rms_dbfs = (rms * 10.0).round() / 10.0;
    r.msps = rate / 1e6;
    r.updated_ms = now_ms();
    r.error = None;
}

/// Stream from an SDR through SoapySDR.
fn read_soapy(ctx: &WideCtx, args: &str, outs: &[SyncSender<Arc<Vec<C32>>>], stop: &AtomicBool) -> Result<bool> {
    let w = &ctx.cfg;
    let settings = crate::sdr::StreamSettings {
        rate: w.rate as f64,
        center_hz: w.center_hz as f64 + w.upconverter_hz,
        bandwidth: w.bandwidth,
        gain: w.gain,
        gains: w.gains.clone(),
        antenna: w.antenna.clone(),
        settings: w.settings.clone(),
    };
    let mut src = match crate::sdr::Source::open(args, &settings) {
        Ok(s) => s,
        Err(e) => {
            crate::live::get().lock().unwrap().receivers.entry(ctx.receiver.id.clone()).or_default().error = Some(format!("{e:#}"));
            return Err(e);
        }
    };
    let mut lvl_t = Instant::now();
    info!("wideband {}: streaming from {args} at {:.1} MS/s, {:.3} MHz", ctx.receiver.id, w.rate as f64 / 1e6, w.center_hz as f64 / 1e6);
    let mut dropped = 0u64;
    let mut last_report = Instant::now();
    let mut v = Vec::with_capacity(BLOCK);
    while !stop.load(Ordering::Relaxed) {
        src.read_block(BLOCK, &mut v)?;
        live_levels(&ctx.receiver.id, &v, w.rate as f64, &mut lvl_t);
        let block = Arc::new(std::mem::replace(&mut v, Vec::with_capacity(BLOCK)));
        for o in outs {
            match o.try_send(block.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => dropped += 1,
                Err(TrySendError::Disconnected(_)) => return Ok(false),
            }
        }
        if dropped > 0 && last_report.elapsed() > Duration::from_secs(10) {
            warn!("wideband {}: CPU can't keep up, dropped {dropped} blocks", ctx.receiver.id);
            dropped = 0;
            last_report = Instant::now();
        }
    }
    Ok(false)
}

// ------------------------------------------------------------------ segments

struct Extractor {
    slot: usize,
    generation: u64,
    ex: UsbExtractor,
}

struct SegCtx {
    idx: usize,
    nco: Nco,
    chain: Chain,
    seg_rate: f64,
    center: f64,
    det: Detector,
    ring: VecDeque<Vec<C32>>,
    ring_len: usize,
    ring_max: usize,
    extractors: Vec<Extractor>,
    channel_id: i64,
    in_rate: f64,
    live_idx: usize,
}

impl SegCtx {
    fn run(
        mut self,
        rx: MpscRx<Arc<Vec<C32>>>,
        mgr: Arc<Mutex<Manager>>,
        gens: Arc<Vec<AtomicU64>>,
        slots: Vec<SyncSender<Vec<i16>>>,
        tx: Sender<Event>,
        stop: Arc<AtomicBool>,
    ) {
        let mut last_expire = Instant::now();
        let mut cov_start = now_ms();
        let mut cov_samples = 0u64;
        let mut last_cov = Instant::now();
        let mut audio = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let block = match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(b) => b,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => return,
            };
            cov_samples += block.len() as u64;
            let mut mixed = block.to_vec();
            self.nco.mix(&mut mixed);
            let mut seg = Vec::with_capacity(mixed.len() / 4 + 1);
            self.chain.process(&mixed, &mut seg);
            if seg.is_empty() {
                continue;
            }
            // ring buffer for pre-roll
            self.ring_len += seg.len();
            self.ring.push_back(seg.clone());
            while self.ring_len > self.ring_max + self.ring.front().map_or(0, |b| b.len()) {
                self.ring_len -= self.ring.pop_front().unwrap().len();
            }
            // existing decoders on this segment
            self.extractors.retain(|e| gens[e.slot].load(Ordering::SeqCst) == e.generation);
            for e in self.extractors.iter_mut() {
                audio.clear();
                e.ex.process(&seg, &mut audio);
                send_audio(&slots[e.slot], &audio);
            }
            // new bursts
            let now = Instant::now();
            let dets = self.det.process(&seg);
            if let Some((db, floor)) = self.det.take_snapshot() {
                let mut l = crate::live::get().lock().unwrap();
                if let Some(s) = l.segments.get_mut(self.live_idx) {
                    s.db = db;
                    s.floor = floor;
                    s.threshold_db = self.det.threshold_db;
                    s.updated_ms = now_ms();
                }
            }
            for d in dets {
                let rf = self.center + d.offset_hz;
                let got = mgr.lock().unwrap().on_detection(self.idx, rf, now);
                if let Some((slot, generation)) = got {
                    info!(
                        "segment {}: burst at {:.0} Hz ({:.0} Hz wide, +{:.1} dB)",
                        self.idx, rf, d.width_hz, d.snr_db
                    );
                    let mut ex = UsbExtractor::new(self.seg_rate, d.offset_hz);
                    // drop any older extractor of this slot on this segment
                    self.extractors.retain(|e| e.slot != slot);
                    audio.clear();
                    for b in &self.ring {
                        ex.process(b, &mut audio);
                    }
                    send_audio(&slots[slot], &audio);
                    self.extractors.push(Extractor { slot, generation, ex });
                }
            }
            if last_expire.elapsed() > Duration::from_secs(1) {
                mgr.lock().unwrap().expire(now);
                last_expire = now;
            }
            if last_cov.elapsed() > Duration::from_secs(30) {
                let t = now_ms();
                // count as listening if most of the expected IQ arrived
                let expected = last_cov.elapsed().as_secs_f64() * self.in_rate;
                if cov_samples as f64 >= 0.5 * expected {
                    let _ = tx.send(Event::Coverage { channel_id: self.channel_id, start_ms: cov_start, end_ms: t });
                }
                cov_start = t;
                cov_samples = 0;
                last_cov = Instant::now();
            }
        }
    }
}

fn send_audio(tx: &SyncSender<Vec<i16>>, audio: &[i16]) {
    for c in audio.chunks(4000) {
        if tx.try_send(c.to_vec()).is_err() {
            break; // decoder behind; it will resync
        }
    }
}

// ------------------------------------------------------------------ decoder slots

struct SlotCtx {
    idx: usize,
    port: u16,
    dir: PathBuf,
    mercury: PathBuf,
    connect_only: bool,
    receiver: Receiver,
    seg_channels: Vec<i64>,
}

impl SlotCtx {
    fn run(self, rx: Arc<Mutex<MpscRx<Vec<i16>>>>, mgr: Arc<Mutex<Manager>>, tx: Sender<Event>, stop: Arc<AtomicBool>) {
        let mut backoff = 2;
        while !stop.load(Ordering::Relaxed) {
            match self.run_once(&rx, &mgr, &tx, &stop) {
                Ok(true) => return, // the wideband source is gone for good
                Ok(false) => {}
                Err(e) => warn!("decoder {}: {e:#}", self.idx),
            }
            for _ in 0..backoff * 10 {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            backoff = (backoff * 2).min(60);
        }
    }

    fn run_once(
        &self,
        rx: &Arc<Mutex<MpscRx<Vec<i16>>>>,
        mgr: &Arc<Mutex<Manager>>,
        tx: &Sender<Event>,
        stop: &Arc<AtomicBool>,
    ) -> Result<bool> {
        std::fs::create_dir_all(&self.dir)?;
        let fifo = self.dir.join("audio.fifo");
        mkfifo(&fifo)?;
        let ini = self.dir.join("mercury.ini");
        std::fs::write(
            &ini,
            format!(
                "; generated by openrms-watch: receive-only monitor\n[main]\nui_enabled = false\n\
                 sound_system = fifo\ninput_device = {}\noutput_device = /dev/null\n\
                 arq_tcp_base_port = {}\nbroadcast_tcp_port = {}\n[ptt]\nmethod = none\n[store]\nenabled = false\n",
                fifo.display(), self.port, self.port + 2
            ),
        )?;
        let log = std::fs::File::create(self.dir.join("mercury.log"))?;
        let _m = spawn_group(
            Command::new(&self.mercury)
                .arg("-C")
                .arg(&ini)
                .current_dir(&self.dir)
                .env("MERCURY_MONITOR_CONNECT_ONLY", if self.connect_only { "1" } else { "0" })
                .env("MERCURY_BIND_ADDR", "127.0.0.1")
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log),
        )?;
        let mut ctl = None;
        for _ in 0..150 {
            if let Ok(s) = TcpStream::connect(("127.0.0.1", self.port)) {
                ctl = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut ctl = ctl.context("mercury control port never opened")?;
        ctl.write_all(b"MONITOR ON\r")?;
        ctl.set_read_timeout(Some(Duration::from_millis(500)))?;

        let local_stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (rx, local_stop, stop) = (rx.clone(), local_stop.clone(), stop.clone());
            // Ok(true): every audio sender is gone (the source finished)
            std::thread::spawn(move || -> Result<bool> {
                let mut out = open_fifo_writer(&fifo, &local_stop)?;
                let rx = rx.lock().unwrap();
                // drop whatever queued while no Mercury was listening
                while rx.try_recv().is_ok() {}
                let mut b = Vec::new();
                while !local_stop.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                    match rx.recv_timeout(Duration::from_millis(300)) {
                        Ok(chunk) => {
                            b.clear();
                            for s in chunk {
                                b.extend_from_slice(&((s as i32) << 16).to_le_bytes());
                            }
                            out.write_all(&b)?;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(_) => return Ok(true),
                    }
                }
                Ok(false)
            })
        };

        let mut rd = BufReader::new(ctl.try_clone()?);
        let mut buf = Vec::new();
        let res = loop {
            if stop.load(Ordering::Relaxed) {
                break Ok(false);
            }
            if writer.is_finished() {
                break Ok(true);
            }
            buf.clear();
            match rd.read_until(b'\r', &mut buf) {
                Ok(0) => break Err(anyhow::anyhow!("mercury closed the control port")),
                Ok(_) => self.emit_line(&String::from_utf8_lossy(&buf), mgr, tx),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => break Err(e.into()),
            }
        };
        // let Mercury finish decoding what it was given before shutting it down
        if matches!(res, Ok(true)) {
            let end = Instant::now() + Duration::from_secs(3);
            while Instant::now() < end {
                buf.clear();
                if let Ok(n) = rd.read_until(b'\r', &mut buf) {
                    if n > 0 {
                        self.emit_line(&String::from_utf8_lossy(&buf), mgr, tx);
                    }
                }
            }
        }
        local_stop.store(true, Ordering::Relaxed);
        drop(_m);
        let finished = matches!(writer.join(), Ok(Ok(true)));
        res.map(|r| r && finished)
    }

    fn emit_line(&self, line: &str, mgr: &Arc<Mutex<Manager>>, tx: &Sender<Event>) {
        let Some(frame) = crate::monitor::parse_line(line) else { return };
        let Some(a) = mgr.lock().unwrap().on_frame(self.idx, &frame.kind, Instant::now()) else { return };
        // report the true frequency: correct the receiver's ppm error
        let rf = a.rf_center * (1.0 + self.receiver.ppm * 1e-6) + self.receiver.offset_hz;
        let dial = ((rf - AUDIO_CENTER) / 10.0).round() as u64 * 10;
        info!("decoder {} @ {} Hz: {}", self.idx, dial, frame.raw);
        let _ = tx.send(Event::Frame(Heard {
            ts_ms: now_ms(),
            receiver_id: self.receiver.id.clone(),
            channel_id: self.seg_channels[a.seg],
            dial_hz: dial,
            shift_hz: 0.0,
            freq_source: "scan",
            frame,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimation_choice() {
        assert_eq!(12_800_000 / seg_decim(12_800_000, 300_000.0) as u64, 400_000);
        assert_eq!(12_800_000 / seg_decim(12_800_000, 350_000.0) as u64, 512_000);
        assert_eq!(1_600_000 / seg_decim(1_600_000, 100_000.0) as u64, 160_000);
    }

    #[test]
    fn manager_assigns_dedups_pins_and_expires() {
        let mut m = Manager::new(2);
        let t = Instant::now();
        let (s0, _) = m.on_detection(0, 7_102_000.0, t).unwrap();
        assert!(m.on_detection(0, 7_102_030.0, t).is_none(), "same frequency reuses the decoder");
        let (s1, _) = m.on_detection(0, 7_150_000.0, t).unwrap();
        assert_ne!(s0, s1);
        // RMS answers on s0 -> pinned; third signal evicts unconfirmed s1, not s0
        m.on_frame(s0, "ACCEPT", t);
        let (s2, _) = m.on_detection(0, 7_200_000.0, t).unwrap();
        assert_eq!(s2, s1);
        // unconfirmed expires, goes on cooldown
        m.expire(t + UNCONFIRMED_HOLD + Duration::from_secs(1));
        assert!(m.slots[s1].assign.is_none());
        assert!(m.slots[s0].assign.is_some(), "pinned stays");
        assert!(m.on_detection(0, 7_200_010.0, t + UNCONFIRMED_HOLD + Duration::from_secs(2)).is_none());
        // after the first cooldown it gets another chance; a second strike holds it 4x longer
        let t2 = t + UNCONFIRMED_HOLD + COOLDOWN + Duration::from_secs(5);
        assert!(m.on_detection(0, 7_200_000.0, t2).is_some());
        m.expire(t2 + UNCONFIRMED_HOLD + Duration::from_secs(1));
        let t3 = t2 + UNCONFIRMED_HOLD + COOLDOWN + Duration::from_secs(5);
        assert!(m.on_detection(0, 7_200_000.0, t3).is_none(), "second strike: longer cooldown");
    }
}
