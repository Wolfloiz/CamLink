//! Pacing de frames por deadline absoluto — lógica pura e cross-platform
//! (testável no Linux) consumida pelo `worker_loop` do filtro DirectShow
//! (`dshow.rs`), onde `thread::sleep(period - spent)` acumulava o erro de
//! granularidade do timer do Windows (~15,6 ms) a cada iteração e derrubava
//! 30 fps para ~15–21.

use std::time::{Duration, Instant};

/// Próximo deadline de frame: `prev + period` (absoluto — o erro de uma
/// espera não contamina as seguintes). Atrasos menores que um período NÃO
/// ressincronizam: o deadline devolvido já passou, o chamador dispara
/// imediatamente e a cadência converge de volta sem drift. Após um stall de
/// um período ou mais (decoder parado, device dormindo), ressincroniza em
/// `now` — descarta o backlog em vez de disparar uma rajada de frames.
pub fn next_frame_deadline(prev: Instant, period: Duration, now: Instant) -> Instant {
    let next = prev + period;
    if now.saturating_duration_since(next) >= period {
        now
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD: Duration = Duration::from_millis(33);

    #[test]
    fn deadlines_do_not_drift_over_many_iterations() {
        let t0 = Instant::now();
        let mut deadline = t0;
        for _ in 0..1000 {
            // Simula acordar exatamente no deadline (espera perfeita).
            deadline = next_frame_deadline(deadline, PERIOD, deadline);
        }
        assert_eq!(
            deadline,
            t0 + PERIOD * 1000,
            "deadline absoluto não pode acumular erro"
        );
    }

    #[test]
    fn small_lateness_keeps_absolute_cadence() {
        let t0 = Instant::now();
        // Acordou 10 ms atrasado (menos que um período): o próximo deadline
        // continua ancorado em t0+period — sem ressincronizar, o atraso é
        // reabsorvido na iteração seguinte.
        let now = t0 + PERIOD / 2;
        assert_eq!(next_frame_deadline(t0, PERIOD, now), t0 + PERIOD);
    }

    #[test]
    fn stall_resyncs_to_now_without_burst() {
        let t0 = Instant::now();
        // Stall de 10 períodos: ressincroniza em `now` — um deadline no
        // passado distante dispararia uma rajada de frames de uma vez.
        let now = t0 + PERIOD * 10;
        assert_eq!(next_frame_deadline(t0, PERIOD, now), now);
    }

    #[test]
    fn stall_boundary_is_exactly_one_period_late() {
        let t0 = Instant::now();
        // Exatamente 1 período de atraso sobre o deadline: já é stall.
        let now = t0 + PERIOD * 2;
        assert_eq!(next_frame_deadline(t0, PERIOD, now), now);
        // Um tick antes disso ainda é atraso pequeno: mantém a âncora.
        let now = t0 + PERIOD * 2 - Duration::from_nanos(1);
        assert_eq!(next_frame_deadline(t0, PERIOD, now), t0 + PERIOD);
    }
}
