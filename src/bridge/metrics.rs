use crate::stats::Metric;
use crossbeam_channel::Sender;
use rquickjs::{Ctx, Function, Object, Result};
use std::collections::HashMap;

fn send_histogram(tx: &Sender<Metric>, name: String, value: f64, tags: HashMap<String, String>) {
    let _ = tx.send(Metric::Histogram { name, value, tags });
}

fn send_counter(tx: &Sender<Metric>, name: String, value: f64, tags: HashMap<String, String>) {
    let _ = tx.send(Metric::Counter { name, value, tags });
}

fn send_gauge(tx: &Sender<Metric>, name: String, value: f64, tags: HashMap<String, String>) {
    let _ = tx.send(Metric::Gauge { name, value, tags });
}

fn send_rate(tx: &Sender<Metric>, name: String, success: bool, tags: HashMap<String, String>) {
    let _ = tx.send(Metric::Rate {
        name,
        success,
        tags,
    });
}

pub fn register_sync<'js>(ctx: &Ctx<'js>, tx: Sender<Metric>) -> Result<()> {
    let metrics = Object::new(ctx.clone())?;

    // histogram.add(name, value, tags?) - Add a value to a named histogram
    let tx_hist = tx.clone();
    metrics.set(
        "histogramAdd",
        Function::new(
            ctx.clone(),
            move |name: String,
                  value: f64,
                  tags: rquickjs::function::Opt<HashMap<String, String>>| {
                send_histogram(&tx_hist, name, value, tags.0.unwrap_or_default());
            },
        ),
    )?;

    // counter.add(name, value, tags?) - Add a value to a named counter
    let tx_counter = tx.clone();
    metrics.set(
        "counterAdd",
        Function::new(
            ctx.clone(),
            move |name: String,
                  value: f64,
                  tags: rquickjs::function::Opt<HashMap<String, String>>| {
                send_counter(&tx_counter, name, value, tags.0.unwrap_or_default());
            },
        ),
    )?;

    // gauge.set(name, value, tags?) - Set the value of a named gauge
    let tx_gauge = tx.clone();
    metrics.set(
        "gaugeSet",
        Function::new(
            ctx.clone(),
            move |name: String,
                  value: f64,
                  tags: rquickjs::function::Opt<HashMap<String, String>>| {
                send_gauge(&tx_gauge, name, value, tags.0.unwrap_or_default());
            },
        ),
    )?;

    // rate.add(name, success, tags?) - Add an observation to a named rate
    let tx_rate = tx.clone();
    metrics.set(
        "rateAdd",
        Function::new(
            ctx.clone(),
            move |name: String,
                  success: bool,
                  tags: rquickjs::function::Opt<HashMap<String, String>>| {
                send_rate(&tx_rate, name, success, tags.0.unwrap_or_default());
            },
        ),
    )?;

    ctx.globals().set("metrics", metrics)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    #[test]
    fn test_send_histogram() {
        let (tx, rx) = unbounded();
        send_histogram(&tx, "response_time".to_string(), 123.45, HashMap::new());

        let metric = rx.recv().unwrap();
        match metric {
            Metric::Histogram { name, value, tags } => {
                assert_eq!(name, "response_time");
                assert!((value - 123.45).abs() < f64::EPSILON);
                assert!(tags.is_empty());
            }
            _ => panic!("Expected Histogram metric"),
        }
    }

    #[test]
    fn test_send_counter() {
        let (tx, rx) = unbounded();
        send_counter(&tx, "requests".to_string(), 1.0, HashMap::new());

        let metric = rx.recv().unwrap();
        match metric {
            Metric::Counter { name, value, tags } => {
                assert_eq!(name, "requests");
                assert!((value - 1.0).abs() < f64::EPSILON);
                assert!(tags.is_empty());
            }
            _ => panic!("Expected Counter metric"),
        }
    }

    #[test]
    fn test_send_gauge() {
        let (tx, rx) = unbounded();
        send_gauge(&tx, "active_users".to_string(), 42.0, HashMap::new());

        let metric = rx.recv().unwrap();
        match metric {
            Metric::Gauge { name, value, tags } => {
                assert_eq!(name, "active_users");
                assert!((value - 42.0).abs() < f64::EPSILON);
                assert!(tags.is_empty());
            }
            _ => panic!("Expected Gauge metric"),
        }
    }

    #[test]
    fn test_send_rate_success() {
        let (tx, rx) = unbounded();
        send_rate(&tx, "check_passed".to_string(), true, HashMap::new());

        let metric = rx.recv().unwrap();
        match metric {
            Metric::Rate {
                name,
                success,
                tags,
            } => {
                assert_eq!(name, "check_passed");
                assert!(success);
                assert!(tags.is_empty());
            }
            _ => panic!("Expected Rate metric"),
        }
    }

    #[test]
    fn test_send_rate_failure() {
        let (tx, rx) = unbounded();
        send_rate(&tx, "check_failed".to_string(), false, HashMap::new());

        let metric = rx.recv().unwrap();
        match metric {
            Metric::Rate {
                name,
                success,
                tags,
            } => {
                assert_eq!(name, "check_failed");
                assert!(!success);
                assert!(tags.is_empty());
            }
            _ => panic!("Expected Rate metric"),
        }
    }

    #[test]
    fn test_multiple_metrics() {
        let (tx, rx) = unbounded();

        send_histogram(&tx, "hist1".to_string(), 1.0, HashMap::new());
        send_counter(&tx, "counter1".to_string(), 2.0, HashMap::new());
        send_gauge(&tx, "gauge1".to_string(), 3.0, HashMap::new());
        send_rate(&tx, "rate1".to_string(), true, HashMap::new());

        // Verify all 4 metrics were sent
        assert!(matches!(rx.recv().unwrap(), Metric::Histogram { .. }));
        assert!(matches!(rx.recv().unwrap(), Metric::Counter { .. }));
        assert!(matches!(rx.recv().unwrap(), Metric::Gauge { .. }));
        assert!(matches!(rx.recv().unwrap(), Metric::Rate { .. }));
    }

    #[test]
    fn test_histogram_negative_value() {
        let (tx, rx) = unbounded();
        send_histogram(&tx, "temp".to_string(), -10.5, HashMap::new());

        let metric = rx.recv().unwrap();
        match metric {
            Metric::Histogram { value, .. } => {
                assert!((value - (-10.5)).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Histogram metric"),
        }
    }

    #[test]
    fn test_counter_large_value() {
        let (tx, rx) = unbounded();
        send_counter(&tx, "bytes".to_string(), 1_000_000_000.0, HashMap::new());

        let metric = rx.recv().unwrap();
        match metric {
            Metric::Counter { value, .. } => {
                assert!((value - 1_000_000_000.0).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Counter metric"),
        }
    }
}
