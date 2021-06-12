use dasp_ring_buffer::Bounded;


pub fn create_noise_gate(chunks: usize, mute_percentage: f32) -> impl FnMut(&mut [f32]) -> Option<&mut [f32]> {
    let mut peaks = Bounded::from_full(vec![0.0; chunks]);
    let mut alltime_high: f32 = 0.0;
    move |buf: &mut [f32]| {
        let max = buf.iter().map(|e| e.abs()).max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap();
        peaks.push(max);
        alltime_high = alltime_high.max(max);
        if peaks.iter().any(|e| *e >= alltime_high * mute_percentage) {
            Some(buf)
        } else {
            None
        }
    }
}
