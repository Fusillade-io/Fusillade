use rquickjs::{Ctx, Class, Result, Value, Function, Object, JsLifetime, class::{Trace, Tracer}};
use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use serde_json::Value as JsonValue;

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
            return Err(rquickjs::Error::new_from_js("SharedData not found in context", "Error"));
        };

        let mut data_opt: Option<Arc<Vec<JsonValue>>> = None;

        // 1. Try Read
        {
            let read = shared_data.read().map_err(|_| rquickjs::Error::new_from_js("SharedData lock poisoned (read)", "Error"))?;
            if let Some(d) = read.get(&name) {
                data_opt = Some(d.clone());
            }
        }

        // 2. Write if missing
        if data_opt.is_none() {
            let mut write = shared_data.write().map_err(|_| rquickjs::Error::new_from_js("SharedData lock poisoned (write)", "Error"))?;
            if let Some(d) = write.get(&name) {
                data_opt = Some(d.clone());
            } else {
                let factory = factory_val.into_function().ok_or_else(|| rquickjs::Error::new_from_js("Factory must be a function", "TypeError"))?;
                
                let res: Value = factory.call(())?;
                
                if !res.is_array() {
                        return Err(rquickjs::Error::new_from_js("Factory must return an array", "TypeError"));
                }
                
                let arr = res.into_array().ok_or_else(|| rquickjs::Error::new_from_js("Factory failed to return array", "TypeError"))?;
                let mut vec = Vec::new();

                let json_obj: Object = ctx.globals().get("JSON")?;
                let stringify: Function = json_obj.get("stringify")?;

                for item in arr.iter() {
                        let item_val: Value = item?;
                        let json_str: String = stringify.call((item_val,))?;
                        let serde_val: JsonValue = serde_json::from_str(&json_str).map_err(|_| rquickjs::Error::new_from_js("String", "JsonValue"))?;
                        vec.push(serde_val);
                }
                
                let arc_vec = Arc::new(vec);
                write.insert(name.clone(), arc_vec.clone());
                data_opt = Some(arc_vec);
            }
        }

        Ok(SharedArray { data: data_opt.ok_or_else(|| rquickjs::Error::new_from_js("Failed to initialize SharedArray", "Error"))? })
    }

    #[qjs(get, rename = "length")]
    pub fn length(&self) -> usize {
        self.data.len()
    }

    #[qjs(rename = "get")]
    pub fn get<'js>(&self, ctx: Ctx<'js>, index: usize) -> Result<Value<'js>> {
         match self.data.get(index) {
             Some(v) => {
                 let s = serde_json::to_string(v).map_err(|_| rquickjs::Error::new_from_js("JsonValue", "String"))?;
                 let json: Object = ctx.globals().get("JSON")?;
                 let parse: Function = json.get("parse")?;
                 parse.call((s,))
             }
             None => Ok(Value::new_undefined(ctx)),
         }
    }
}

pub fn register_sync(ctx: &Ctx, shared_data: SharedData) -> Result<()> {
    // Store shared_data in context so constructor can access it
    let _ = ctx.store_userdata(SharedDataWrapper(shared_data));

    // Register the class (which includes the constructor)
    Class::<SharedArray>::define(&ctx.globals())?;
    
    Ok(())
}
