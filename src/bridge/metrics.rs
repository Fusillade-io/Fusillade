use rquickjs::{Ctx, Function, Object, Result};
use crossbeam_channel::Sender;
use crate::stats::Metric;

pub fn register_sync<'js>(ctx: &Ctx<'js>, tx: Sender<Metric>) -> Result<()> {
    let metrics = Object::new(ctx.clone())?;

    // histogram.add(name, value) - Add a value to a named histogram
    let tx_hist = tx.clone();
    metrics.set("histogramAdd", Function::new(ctx.clone(), move |name: String, value: f64| {
        let _ = tx_hist.send(Metric::Histogram { name, value, tags: std::collections::HashMap::new() });
    }))?;

    // counter.add(name, value) - Add a value to a named counter
    let tx_counter = tx.clone();
    metrics.set("counterAdd", Function::new(ctx.clone(), move |name: String, value: f64| {
        let _ = tx_counter.send(Metric::Counter { name, value, tags: std::collections::HashMap::new() });
    }))?;

    // gauge.set(name, value) - Set the value of a named gauge
    let tx_gauge = tx.clone();
    metrics.set("gaugeSet", Function::new(ctx.clone(), move |name: String, value: f64| {
        let _ = tx_gauge.send(Metric::Gauge { name, value, tags: std::collections::HashMap::new() });
    }))?;

    // rate.add(name, success) - Add an observation to a named rate
    let tx_rate = tx.clone();
    metrics.set("rateAdd", Function::new(ctx.clone(), move |name: String, success: bool| {
        let _ = tx_rate.send(Metric::Rate { name, success, tags: std::collections::HashMap::new() });
    }))?;

    ctx.globals().set("metrics", metrics)?;
    Ok(())
}
