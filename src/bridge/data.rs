use rquickjs::{
    class::{Trace, Tracer},
    Class, Ctx, Function, JsLifetime, Object, Result, Value,
};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// Shared data storage: Map<Name, Data>
pub type SharedData = Arc<RwLock<HashMap<String, Arc<Vec<JsonValue>>>>>;

// Wrapper for UserData to satisfy JsLifetime
#[derive(Clone)]
pub struct SharedDataWrapper(pub SharedData);

unsafe impl<'js> JsLifetime<'js> for SharedDataWrapper {
    type Changed<'to> = SharedDataWrapper;
}

#[derive(Clone)]
#[rquickjs::class]
pub struct SharedArray {
    data: Arc<Vec<JsonValue>>,
}

impl<'js> Trace<'js> for SharedArray {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {
        // No JS values to trace
    }
}

unsafe impl<'js> JsLifetime<'js> for SharedArray {
    type Changed<'to> = SharedArray;
}

#[rquickjs::methods]
impl SharedArray {
    #[qjs(constructor)]
    pub fn new<'js>(ctx: Ctx<'js>, name: String, factory_val: Value<'js>) -> Result<SharedArray> {
        // Retrieve SharedData from context userdata
        let shared_data = if let Some(wrapper) = ctx.userdata::<SharedDataWrapper>() {
            wrapper.0.clone()
        } else {
            return Err(rquickjs::Error::new_from_js(
                "SharedData not found in context",
                "Error",
            ));
        };

        let mut data_opt: Option<Arc<Vec<JsonValue>>> = None;

        // 1. Try Read
        {
            let read = shared_data.read().map_err(|_| {
                rquickjs::Error::new_from_js("SharedData lock poisoned (read)", "Error")
            })?;
            if let Some(d) = read.get(&name) {
                data_opt = Some(d.clone());
            }
        }

        // 2. Write if missing
        if data_opt.is_none() {
            let mut write = shared_data.write().map_err(|_| {
                rquickjs::Error::new_from_js("SharedData lock poisoned (write)", "Error")
            })?;
            if let Some(d) = write.get(&name) {
                data_opt = Some(d.clone());
            } else {
                let factory = factory_val.into_function().ok_or_else(|| {
                    rquickjs::Error::new_from_js("Factory must be a function", "TypeError")
                })?;

                let res: Value = factory.call(())?;

                if !res.is_array() {
                    return Err(rquickjs::Error::new_from_js(
                        "Factory must return an array",
                        "TypeError",
                    ));
                }

                let arr = res.into_array().ok_or_else(|| {
                    rquickjs::Error::new_from_js("Factory failed to return array", "TypeError")
                })?;
                let mut vec = Vec::new();

                let json_obj: Object = ctx.globals().get("JSON")?;
                let stringify: Function = json_obj.get("stringify")?;

                for item in arr.iter() {
                    let item_val: Value = item?;
                    let json_str: String = stringify.call((item_val,))?;
                    let serde_val: JsonValue = serde_json::from_str(&json_str)
                        .map_err(|_| rquickjs::Error::new_from_js("String", "JsonValue"))?;
                    vec.push(serde_val);
                }

                let arc_vec = Arc::new(vec);
                write.insert(name.clone(), arc_vec.clone());
                data_opt = Some(arc_vec);
            }
        }

        Ok(SharedArray {
            data: data_opt.ok_or_else(|| {
                rquickjs::Error::new_from_js("Failed to initialize SharedArray", "Error")
            })?,
        })
    }

    #[qjs(get, rename = "length")]
    pub fn length(&self) -> usize {
        self.data.len()
    }

    #[qjs(rename = "get")]
    pub fn get<'js>(&self, ctx: Ctx<'js>, index: usize) -> Result<Value<'js>> {
        match self.data.get(index) {
            Some(v) => {
                let s = serde_json::to_string(v)
                    .map_err(|_| rquickjs::Error::new_from_js("JsonValue", "String"))?;
                let json: Object = ctx.globals().get("JSON")?;
                let parse: Function = json.get("parse")?;
                parse.call((s,))
            }
            None => Ok(Value::new_undefined(ctx)),
        }
    }
}

// CSV row type: header names -> values
pub type CsvData = Arc<RwLock<HashMap<String, Arc<(Vec<String>, Vec<Vec<String>>)>>>>;

#[derive(Clone)]
pub struct CsvDataWrapper(pub CsvData);

unsafe impl<'js> JsLifetime<'js> for CsvDataWrapper {
    type Changed<'to> = CsvDataWrapper;
}

#[derive(Clone)]
#[rquickjs::class]
pub struct SharedCSV {
    headers: Vec<String>,
    rows: Arc<Vec<Vec<String>>>,
}

impl<'js> Trace<'js> for SharedCSV {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {
        // No JS values to trace
    }
}

unsafe impl<'js> JsLifetime<'js> for SharedCSV {
    type Changed<'to> = SharedCSV;
}

#[rquickjs::methods]
impl SharedCSV {
    #[qjs(constructor)]
    pub fn new<'js>(ctx: Ctx<'js>, path: String) -> Result<SharedCSV> {
        // Retrieve CsvData from context userdata
        let csv_data = if let Some(wrapper) = ctx.userdata::<CsvDataWrapper>() {
            wrapper.0.clone()
        } else {
            return Err(rquickjs::Error::new_from_js(
                "CsvData not found in context",
                "Error",
            ));
        };

        let mut data_opt: Option<Arc<(Vec<String>, Vec<Vec<String>>)>> = None;

        // 1. Try Read
        {
            let read = csv_data.read().map_err(|_| {
                rquickjs::Error::new_from_js("CsvData lock poisoned (read)", "Error")
            })?;
            if let Some(d) = read.get(&path) {
                data_opt = Some(d.clone());
            }
        }

        // 2. Parse CSV if missing
        if data_opt.is_none() {
            let mut write = csv_data.write().map_err(|_| {
                rquickjs::Error::new_from_js("CsvData lock poisoned (write)", "Error")
            })?;
            if let Some(d) = write.get(&path) {
                data_opt = Some(d.clone());
            } else {
                // Read and parse CSV file
                let content = std::fs::read_to_string(&path).map_err(|_| {
                    rquickjs::Error::new_from_js("Failed to read CSV file", "IOError")
                })?;

                let mut lines = content.lines();
                let headers: Vec<String> = lines
                    .next()
                    .ok_or_else(|| rquickjs::Error::new_from_js("CSV file is empty", "Error"))?
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').to_string())
                    .collect();

                let rows: Vec<Vec<String>> = lines
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| {
                        line.split(',')
                            .map(|s| s.trim().trim_matches('"').to_string())
                            .collect()
                    })
                    .collect();

                let arc_data = Arc::new((headers, rows));
                write.insert(path.clone(), arc_data.clone());
                data_opt = Some(arc_data);
            }
        }

        let data = data_opt.ok_or_else(|| {
            rquickjs::Error::new_from_js("Failed to initialize SharedCSV", "Error")
        })?;
        Ok(SharedCSV {
            headers: data.0.clone(),
            rows: Arc::new(data.1.clone()),
        })
    }

    #[qjs(get, rename = "length")]
    pub fn length(&self) -> usize {
        self.rows.len()
    }

    #[qjs(get, rename = "headers")]
    pub fn get_headers<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let arr = rquickjs::Array::new(ctx.clone())?;
        for (i, h) in self.headers.iter().enumerate() {
            arr.set(i, h.clone())?;
        }
        Ok(arr.into_value())
    }

    /// Get row by index as object with header keys
    #[qjs(rename = "get")]
    pub fn get<'js>(&self, ctx: Ctx<'js>, index: usize) -> Result<Value<'js>> {
        match self.rows.get(index) {
            Some(row) => {
                let obj = Object::new(ctx.clone())?;
                for (i, header) in self.headers.iter().enumerate() {
                    if let Some(value) = row.get(i) {
                        obj.set(header.clone(), value.clone())?;
                    }
                }
                Ok(obj.into_value())
            }
            None => Ok(Value::new_undefined(ctx)),
        }
    }

    /// Get random row
    #[qjs(rename = "random")]
    pub fn random<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        if self.rows.is_empty() {
            return Ok(Value::new_undefined(ctx));
        }
        let index = rand::random::<usize>() % self.rows.len();
        self.get(ctx, index)
    }
}

pub fn register_sync(ctx: &Ctx, shared_data: SharedData) -> Result<()> {
    // Store shared_data in context so constructor can access it
    let _ = ctx.store_userdata(SharedDataWrapper(shared_data));

    // Store CSV data in context
    let csv_data: CsvData = Arc::new(RwLock::new(HashMap::new()));
    let _ = ctx.store_userdata(CsvDataWrapper(csv_data));

    // Register the classes (which includes the constructors)
    Class::<SharedArray>::define(&ctx.globals())?;
    Class::<SharedCSV>::define(&ctx.globals())?;

    Ok(())
}
