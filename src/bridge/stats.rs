use rquickjs::{Ctx, Object, Result, Function};
use crate::stats::SharedAggregator;

pub fn register_sync<'js>(ctx: &Ctx<'js>, aggregator: SharedAggregator) -> Result<()> {
    let stats = Object::new(ctx.clone())?;
    
    // Store aggregator in context userdata to manage its lifetime correctly
    ctx.store_userdata(aggregator)?;

    stats.set("get", Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String| -> Result<Object<'js>> {
        let res = Object::new(ctx.clone())?;
        
        let agg_guard = ctx.userdata::<SharedAggregator>().expect("Aggregator missing in userdata");
        let agg: &SharedAggregator = &agg_guard;
        
        if let Ok(agg) = agg.read() {
            if let Some(s) = agg.requests.get(&name) {
                let micros = s.histogram.value_at_quantile(0.95);
                res.set("p95", micros as f64 / 1000.0)?;
                res.set("p99", s.histogram.value_at_quantile(0.99) as f64 / 1000.0)?;
                res.set("avg", s.total_duration.as_millis() as f64 / s.total_requests.max(1) as f64)?;
                res.set("count", s.total_requests)?;
                res.set("min", s.min_duration.unwrap_or_default().as_millis() as f64)?;
                res.set("max", s.max_duration.as_millis() as f64)?;
            } else {
                res.set("p95", 0.0)?;
                res.set("p99", 0.0)?;
                res.set("avg", 0.0)?;
                res.set("count", 0)?;
                res.set("min", 0.0)?;
                res.set("max", 0.0)?;
            }
        }
        Ok(res)
    }))?;

    ctx.globals().set("stats", stats)?;
    Ok(())
}
