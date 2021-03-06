//! Logic handling the intermediate representation of Avro values.
use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::BuildHasher;
use std::u8;

use failure::Error;
use serde_json::Value as JsonValue;

use crate::LruLimit;
use crate::schema::{RecordField, Schema, SchemaKind, UnionSchema};

const ACCESS_TIME: &str = "access_time";
const COUNT: &str = "count";

lazy_static! {
    pub static ref LRU_VALUE_SCHEMA: Schema = Schema::parse_str(
            r#"
            {
                "type": "record",
                "name": "lru_value",
                "fields": [
                    {"access_time": "a", "type": "long", "default": 0},
                    {"count": "b", "type": "long", "default": 0}
                ]
            }
        "#,
        ).unwrap();
}

/// Describes errors happened while performing schema resolution on Avro data.
#[derive(Fail, Debug)]
#[fail(display = "Decoding error: {}", _0)]
pub struct SchemaResolutionError(String);

impl SchemaResolutionError {
    pub fn new<S>(msg: S) -> SchemaResolutionError
        where
            S: Into<String>,
    {
        SchemaResolutionError(msg.into())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValueSetting {
    pub index: bool
}

/// Represents any valid Avro value
/// More information about Avro values can be found in the
/// [Avro Specification](https://avro.apache.org/docs/current/spec.html#schemas)
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A `null` Avro value.
    Null,

    /// A `boolean` Avro value.
    Boolean(bool, Option<ValueSetting>),

    /// A `int` Avro value.
    Int(i32, Option<ValueSetting>),

    /// A `long` Avro value.
    Long(i64, Option<ValueSetting>),

    /// A `float` Avro value.
    Float(f32, Option<ValueSetting>),

    /// A `double` Avro value.
    Double(f64, Option<ValueSetting>),

    /// A `bytes` Avro value.
    Bytes(Vec<u8>, Option<ValueSetting>),

    /// A `string` Avro value.
    String(String, Option<ValueSetting>),

    /// A `fixed` Avro value.
    /// The size of the fixed value is represented as a `usize`.
    Fixed(usize, Vec<u8>, Option<ValueSetting>),

    /// An `enum` Avro value.
    ///
    /// An Enum is represented by a symbol and its position in the symbols list
    /// of its corresponding schema.
    /// This allows schema-less encoding, as well as schema resolution while
    /// reading values.
    Enum(i32, String, Option<ValueSetting>),

    /// An `union` Avro value.
    Union(Box<Value>, Option<ValueSetting>),

    /// An `array` Avro value.
    Array(Vec<Value>, Option<ValueSetting>),

    /// A `map` Avro value.
    Map(HashMap<String, Value>, Option<ValueSetting>),

    /// A `record` Avro value.
    ///
    /// A Record is represented by a vector of (`<record name>`, `value`).
    /// This allows schema-less encoding.
    ///
    /// See [Record](types.Record) for a more user-friendly support.
    Record(Vec<(String, Value)>, Option<ValueSetting>),

    // date is serialized as millis since epoch
    Date(i64, Option<ValueSetting>),

    // hashset of values
    Set(HashSet<String>, Option<ValueSetting>),

    // vector of value, access time, counts
    LruSet(HashMap<String, LruValue>, LruLimit, Option<ValueSetting>),

    Optional(Option<Box<Value>>, Option<ValueSetting>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LruValue {
    pub access_time: i64,
    pub count: i64,
}

impl LruValue {
    pub fn new(access_time: i64, count: i64) -> LruValue {
        LruValue {
            access_time,
            count,
        }
    }

    fn json(&self) -> JsonValue {
        let mut map: serde_json::Map<String, JsonValue> = serde_json::Map::new();
        map.insert(ACCESS_TIME.to_owned(), json!(self.access_time));
        map.insert(COUNT.to_owned(), json!(self.count));

        JsonValue::Object(map)
    }
}

impl ToAvro for LruValue {
    fn avro(self) -> Value {
        let mut fields: Vec<(String, Value)> = Vec::new();
        fields.push((ACCESS_TIME.to_owned(), Value::Long(self.access_time as i64, None)));
        fields.push((COUNT.to_owned(), Value::Long(self.count as i64, None)));

        Value::Record(fields, None)
    }
}

/// Any structure implementing the [ToAvro](trait.ToAvro.html) trait will be usable
/// from a [Writer](../writer/struct.Writer.html).
pub trait ToAvro {
    /// Transforms this value into an Avro-compatible [Value](enum.Value.html).
    fn avro(self) -> Value;
}

macro_rules! to_avro (
    ($t:ty, $v:expr) => (
        impl ToAvro for $t {
            fn avro(self) -> Value {
                $v(self, None)
            }
        }
    );
);

to_avro!(bool, Value::Boolean);
to_avro!(i32, Value::Int);
to_avro!(i64, Value::Long);
to_avro!(f32, Value::Float);
to_avro!(f64, Value::Double);
to_avro!(String, Value::String);

impl ToAvro for () {
    fn avro(self) -> Value {
        Value::Null
    }
}

impl ToAvro for usize {
    fn avro(self) -> Value {
        (self as i64).avro()
    }
}

impl<'a> ToAvro for &'a str {
    fn avro(self) -> Value {
        Value::String(self.to_owned(), None)
    }
}

impl<'a> ToAvro for &'a [u8] {
    fn avro(self) -> Value {
        Value::Bytes(self.to_owned(), None)
    }
}

impl<T> ToAvro for Option<T>
    where
        T: ToAvro,
{
    fn avro(self) -> Value {
        let v = match self {
            Some(v) => T::avro(v),
            None => Value::Null,
        };
        Value::Union(Box::new(v), None)
    }
}

impl<T, S: BuildHasher> ToAvro for HashMap<String, T, S>
    where
        T: ToAvro,
{
    fn avro(self) -> Value {
        Value::Map(
            self.into_iter()
                .map(|(key, value)| (key, value.avro()))
                .collect::<_>(),
            None,
        )
    }
}

impl<'a, T, S: BuildHasher> ToAvro for HashMap<&'a str, T, S>
    where
        T: ToAvro,
{
    fn avro(self) -> Value {
        Value::Map(
            self.into_iter()
                .map(|(key, value)| (key.to_owned(), value.avro()))
                .collect::<_>(),
            None,
        )
    }
}

impl ToAvro for Value {
    fn avro(self) -> Value {
        self
    }
}

/*
impl<S: Serialize> ToAvro for S {
    fn avro(self) -> Value {
        use ser::Serializer;

        self.serialize(&mut Serializer::new()).unwrap()
    }
}
*/

/// Utility interface to build `Value::Record` objects.
#[derive(Debug, Clone)]
pub struct Record<'a> {
    /// List of fields contained in the record.
    /// Ordered according to the fields in the schema given to create this
    /// `Record` object. Any unset field defaults to `Value::Null`.
    pub fields: Vec<(String, Value)>,
    schema_lookup: &'a HashMap<String, usize>,
}

impl<'a> Record<'a> {
    /// Create a `Record` given a `Schema`.
    ///
    /// If the `Schema` is not a `Schema::Record` variant, `None` will be returned.
    pub fn new(schema: &Schema) -> Option<Record> {
        match *schema {
            Schema::Record {
                fields: ref schema_fields,
                lookup: ref schema_lookup,
                ..
            } => {
                let mut fields = Vec::with_capacity(schema_fields.len());
                for schema_field in schema_fields.iter() {
                    fields.push((schema_field.name.clone(), Value::Null));
                }

                Some(Record {
                    fields,
                    schema_lookup,
                })
            }
            _ => None,
        }
    }

    /// Put a compatible value (implementing the `ToAvro` trait) in the
    /// `Record` for a given `field` name.
    ///
    /// **NOTE** Only ensure that the field name is present in the `Schema` given when creating
    /// this `Record`. Does not perform any schema validation.
    pub fn put<V>(&mut self, field: &str, value: V)
        where
            V: ToAvro,
    {
        if let Some(&position) = self.schema_lookup.get(field) {
            self.fields[position].1 = value.avro()
        }
    }
}

impl<'a> ToAvro for Record<'a> {
    fn avro(self) -> Value {
        Value::Record(self.fields, None)
    }
}

impl ToAvro for JsonValue {
    fn avro(self) -> Value {
        match self {
            JsonValue::Null => Value::Null,
            JsonValue::Bool(b) => Value::Boolean(b, None),
            JsonValue::Number(ref n) if n.is_i64() => Value::Long(n.as_i64().unwrap(), None),
            JsonValue::Number(ref n) if n.is_f64() => Value::Double(n.as_f64().unwrap(), None),
            JsonValue::Number(n) => Value::Long(n.as_u64().unwrap() as i64, None), // Not so great
            JsonValue::String(s) => Value::String(s, None),
            JsonValue::Array(items) => {
                Value::Array(items.into_iter().map(|item| item.avro()).collect::<_>(), None)
            }
            JsonValue::Object(items) => Value::Map(
                items
                    .into_iter()
                    .map(|(key, value)| (key, value.avro()))
                    .collect::<_>(),
                None,
            ),

            // there is no equivalent of set and lru_set in json
            // set in json would be represented as Array
            // while lru_set in json would be represented as Object
        }
    }
}

impl Value {
    /// Validate the value against the given [Schema](../schema/enum.Schema.html).
    ///
    /// See the [Avro specification](https://avro.apache.org/docs/current/spec.html)
    /// for the full set of rules of schema validation.
    pub fn validate(&self, schema: &Schema) -> bool {
        match (self, schema) {
            (&Value::Null, &Schema::Null) => true,
            (&Value::Boolean(_, _), &Schema::Boolean) => true,
            (&Value::Int(_, _), &Schema::Int) => true,
            (&Value::Long(_, _), &Schema::Long) => true,
            (&Value::Float(_, _), &Schema::Float) => true,
            (&Value::Double(_, _), &Schema::Double) => true,
            (&Value::Bytes(_, _), &Schema::Bytes) => true,
            (&Value::String(_, _), &Schema::String) => true,
            (&Value::Fixed(n, _, _), &Schema::Fixed { size, .. }) => n == size,
            (&Value::String(ref s, _), &Schema::Enum { ref symbols, .. }) => symbols.contains(s),
            (&Value::Enum(i, ref s, _), &Schema::Enum { ref symbols, .. }) => symbols
                .get(i as usize)
                .map(|ref symbol| symbol == &s)
                .unwrap_or(false),
            // (&Value::Union(None), &Schema::Union(_)) => true,
            (&Value::Union(ref value, _), &Schema::Union(ref inner)) => {
                inner.find_schema(value).is_some()
            }
            (&Value::Array(ref items, _), &Schema::Array(ref inner)) => {
                items.iter().all(|item| item.validate(inner))
            }
            (&Value::Map(ref items, _), &Schema::Map(ref inner)) => {
                items.iter().all(|(_, value)| value.validate(inner))
            }
            (&Value::Record(ref record_fields, _), &Schema::Record { ref fields, .. }) => {
                // handle case of missing fields from value
                fields.len() == record_fields.len() && fields.iter().zip(record_fields.iter()).all(
                    |(field, &(ref name, ref value))| {
                        field.name == *name && value.validate(&field.schema)
                    },
                )
            }

            (&Value::Date(ref value, _), &Schema::Date) => {
                // if value can be represented as u4, then it's a valid Date
                true
            }
            (&Value::Set(ref items, _), &Schema::Set) => {
                // if value could be represented as typed HashSet of String, then no further validations are required.
                true
            }
            (&Value::LruSet(ref items, _, _), &Schema::LruSet(ref lru_limit)) => {
                // if value could be represented as typed HashMap of String and LruValue, then no further validations are required.
                true
            }
            (&Value::Optional(ref value, _), &Schema::Optional(ref inner)) => {
                match value {
                    Some(v) => v.validate(inner),
                    None => true
                }
            }

            _ => false,
        }
    }

    /// Attempt to perform schema resolution on the value, with the given
    /// [Schema](../schema/enum.Schema.html).
    ///
    /// See [Schema Resolution](https://avro.apache.org/docs/current/spec.html#Schema+Resolution)
    /// in the Avro specification for the full set of rules of schema
    /// resolution.
    pub fn resolve(mut self, schema: &Schema) -> Result<Self, Error> {
        // Check if this schema is a union, and if the reader schema is not.
        if SchemaKind::from(&self) == SchemaKind::Union
            && SchemaKind::from(schema) != SchemaKind::Union
        {
            // Pull out the Union, and attempt to resolve against it.
            let v = match self {
                Value::Union(b, _) => *b,
                _ => unreachable!(),
            };
            self = v;
        }
        match *schema {
            Schema::Null => self.resolve_null(),
            Schema::Boolean => self.resolve_boolean(false),
            Schema::Int => self.resolve_int(false),
            Schema::Long => self.resolve_long(false),
            Schema::Float => self.resolve_float(false),
            Schema::Double => self.resolve_double(false),
            Schema::Bytes => self.resolve_bytes(false),
            Schema::String => self.resolve_string(false),
            Schema::Fixed { size, .. } => self.resolve_fixed(size, false),
            Schema::Union(ref inner) => self.resolve_union(inner, false),
            Schema::Enum { ref symbols, .. } => self.resolve_enum(symbols, false),
            Schema::Array(ref inner) => self.resolve_array(inner, false),
            Schema::Map(ref inner) => self.resolve_map(inner, false),
            Schema::Record { ref name, ref fields, .. } => self.resolve_record(fields, name.index),

            Schema::Date => self.resolve_datetime(false),
            Schema::Set => self.resolve_set(false),
            Schema::LruSet(ref lru_limit) => self.resolve_lru_set(lru_limit.clone(), false),
            Schema::Optional(ref inner) => self.resolve_optional(inner, false),
        }
    }

    /// Attempt to perform schema resolution on the value, with the given
    /// [Schema](../schema/enum.Schema.html).
    ///
    /// See [Schema Resolution](https://avro.apache.org/docs/current/spec.html#Schema+Resolution)
    /// in the Avro specification for the full set of rules of schema
    /// resolution.
    fn resolve_internal(mut self, schema: &Schema, index: bool) -> Result<Self, Error> {
        // Check if this schema is a union, and if the reader schema is not.
        if SchemaKind::from(&self) == SchemaKind::Union
            && SchemaKind::from(schema) != SchemaKind::Union
        {
            // Pull out the Union, and attempt to resolve against it.
            let v = match self {
                Value::Union(b, _) => *b,
                _ => unreachable!(),
            };
            self = v;
        }
        match *schema {
            Schema::Null => self.resolve_null(),
            Schema::Boolean => self.resolve_boolean(index),
            Schema::Int => self.resolve_int(false),
            Schema::Long => self.resolve_long(false),
            Schema::Float => self.resolve_float(false),
            Schema::Double => self.resolve_double(false),
            Schema::Bytes => self.resolve_bytes(false),
            Schema::String => self.resolve_string(index),
            Schema::Fixed { size, .. } => self.resolve_fixed(size, false),
            Schema::Union(ref inner) => self.resolve_union(inner, false),
            Schema::Enum { ref symbols, .. } => self.resolve_enum(symbols, index),
            Schema::Array(ref inner) => self.resolve_array(inner, index),
            Schema::Map(ref inner) => self.resolve_map(inner, index),
            Schema::Record { ref name, ref fields, .. } => self.resolve_record(fields, name.index),

            Schema::Date => self.resolve_datetime(index),
            Schema::Set => self.resolve_set(index),
            Schema::LruSet(ref lru_limit) => self.resolve_lru_set(lru_limit.clone(), index),
            Schema::Optional(ref inner) => self.resolve_optional(inner, index),
        }
    }

    fn resolve_null(self) -> Result<Self, Error> {
        match self {
            Value::Null => Ok(Value::Null),
            other => {
                Err(SchemaResolutionError::new(format!("Null expected, got {:?}", other)).into())
            }
        }
    }

    fn resolve_boolean(self, index: bool) -> Result<Self, Error> {
        match self {
            Value::Boolean(b, _) => Ok(Value::Boolean(b, Self::get_value_setting(index))),
            other => {
                Err(SchemaResolutionError::new(format!("Boolean expected, got {:?}", other)).into())
            }
        }
    }

    fn resolve_int(self, index: bool) -> Result<Self, Error> {
        match self {
            Value::Int(n, _) => Ok(Value::Int(n, Self::get_value_setting(index))),
            Value::Long(n, _) => Ok(Value::Int(n as i32, Self::get_value_setting(index))),
            other => {
                Err(SchemaResolutionError::new(format!("Int expected, got {:?}", other)).into())
            }
        }
    }

    fn resolve_long(self, index: bool) -> Result<Self, Error> {
        match self {
            Value::Int(n, _) => Ok(Value::Long(i64::from(n), Self::get_value_setting(index))),
            Value::Long(n, _) => Ok(Value::Long(n, Self::get_value_setting(index))),
            other => {
                Err(SchemaResolutionError::new(format!("Long expected, got {:?}", other)).into())
            }
        }
    }

    fn resolve_float(self, index: bool) -> Result<Self, Error> {
        match self {
            Value::Int(n, _) => Ok(Value::Float(n as f32, Self::get_value_setting(index))),
            Value::Long(n, _) => Ok(Value::Float(n as f32, Self::get_value_setting(index))),
            Value::Float(x, _) => Ok(Value::Float(x, Self::get_value_setting(index))),
            Value::Double(x, _) => Ok(Value::Float(x as f32, Self::get_value_setting(index))),
            other => {
                Err(SchemaResolutionError::new(format!("Float expected, got {:?}", other)).into())
            }
        }
    }

    fn resolve_double(self, index: bool) -> Result<Self, Error> {
        match self {
            Value::Int(n, _) => Ok(Value::Double(f64::from(n), Self::get_value_setting(index))),
            Value::Long(n, _) => Ok(Value::Double(n as f64, Self::get_value_setting(index))),
            Value::Float(x, _) => Ok(Value::Double(f64::from(x), Self::get_value_setting(index))),
            Value::Double(x, _) => Ok(Value::Double(x, Self::get_value_setting(index))),
            other => {
                Err(SchemaResolutionError::new(format!("Double expected, got {:?}", other)).into())
            }
        }
    }

    fn resolve_bytes(self, index: bool) -> Result<Self, Error> {
        match self {
            Value::Bytes(bytes, _) => Ok(Value::Bytes(bytes, Self::get_value_setting(index))),
            Value::String(s, _) => Ok(Value::Bytes(s.into_bytes(), Self::get_value_setting(index))),
            Value::Array(items, _) => Ok(Value::Bytes(
                items
                    .into_iter()
                    .map(Value::try_u8)
                    .collect::<Result<Vec<_>, _>>()?,
                Self::get_value_setting(index),
            )),
            other => {
                Err(SchemaResolutionError::new(format!("Bytes expected, got {:?}", other)).into())
            }
        }
    }

    fn resolve_string(self, index: bool) -> Result<Self, Error> {
        match self {
            Value::String(s, _) => Ok(Value::String(s, Self::get_value_setting(index))),
            Value::Bytes(bytes, _) => Ok(Value::String(String::from_utf8(bytes)?, Self::get_value_setting(index))),
            other => {
                Err(SchemaResolutionError::new(format!("String expected, got {:?}", other)).into())
            }
        }
    }

    fn resolve_fixed(self, size: usize, index: bool) -> Result<Self, Error> {
        match self {
            Value::Fixed(n, bytes, _) => if n == size {
                Ok(Value::Fixed(n, bytes, Self::get_value_setting(index)))
            } else {
                Err(SchemaResolutionError::new(format!(
                    "Fixed size mismatch, {} expected, got {}",
                    size, n
                )).into())
            },
            other => {
                Err(SchemaResolutionError::new(format!("String expected, got {:?}", other)).into())
            }
        }
    }

    fn resolve_enum(self, symbols: &[String], index: bool) -> Result<Self, Error> {
        let validate_symbol = |symbol: String, symbols: &[String]| {
            if let Some(i) = symbols.iter().position(|ref item| item == &&symbol) {
                Ok(Value::Enum(i as i32, symbol, Self::get_value_setting(index)))
            } else {
                Err(SchemaResolutionError::new(format!(
                    "Enum default {} is not among allowed symbols {:?}",
                    symbol, symbols,
                )).into())
            }
        };

        match self {
            Value::Enum(i, s, _) => if i >= 0 && i < symbols.len() as i32 {
                validate_symbol(s, symbols)
            } else {
                Err(SchemaResolutionError::new(format!(
                    "Enum value {} is out of bound {}",
                    i,
                    symbols.len() as i32
                )).into())
            },
            Value::String(s, _) => validate_symbol(s, symbols),
            other => Err(SchemaResolutionError::new(format!(
                "Enum({:?}) expected, got {:?}",
                symbols, other
            )).into()),
        }
    }

    fn resolve_union(self, schema: &UnionSchema, index: bool) -> Result<Self, Error> {
        let v = match self {
            // Both are unions case.
            Value::Union(v, _) => *v,
            // Reader is a union, but writer is not.
            v => v,
        };
        // Find the first match in the reader schema.
        let (_, inner) = schema
            .find_schema(&v)
            .ok_or_else(|| SchemaResolutionError::new("Could not find matching type in union"))?;
        v.resolve_internal(inner, index)
    }

    fn resolve_array(self, schema: &Schema, index: bool) -> Result<Self, Error> {
        match self {
            Value::Array(items, _) => Ok(Value::Array(
                items
                    .into_iter()
                    .map(|item| item.resolve_internal(schema, index))
                    .collect::<Result<Vec<_>, _>>()?,
                Self::get_value_setting(index),
            )),
            other => Err(SchemaResolutionError::new(format!(
                "Array({:?}) expected, got {:?}",
                schema, other
            )).into()),
        }
    }

    fn resolve_map(self, schema: &Schema, index: bool) -> Result<Self, Error> {
        match self {
            Value::Map(items, _) => Ok(Value::Map(
                items
                    .into_iter()
                    .map(|(key, value)| value.resolve_internal(schema, index).map(|value| (key, value)))
                    .collect::<Result<HashMap<_, _>, _>>()?,
                Self::get_value_setting(index),
            )),
            other => Err(SchemaResolutionError::new(format!(
                "Map({:?}) expected, got {:?}",
                schema, other
            )).into()),
        }
    }

    fn resolve_record(self, fields: &[RecordField], index: bool) -> Result<Self, Error> {
        let mut items = match self {
            Value::Map(items, _) => Ok(items),
            Value::Record(fields, _) => Ok(fields.into_iter().collect::<HashMap<_, _>>()),
            other => Err(SchemaResolutionError::new(format!(
                "Record({:?}) expected, got {:?}",
                fields, other
            ))),
        }?;

        let new_fields = fields
            .iter()
            .map(|field| {
                let value = match items.remove(&field.name) {
                    Some(value) => value,
                    None => match field.default {
                        Some(ref value) => match field.schema {
                            Schema::Enum { ref symbols, .. } => {
                                value.clone().avro().resolve_enum(symbols, field.index)?
                            }
                            _ => value.clone().avro(),
                        },
                        _ => {
                            return Err(SchemaResolutionError::new(format!(
                                "missing field {} in record",
                                field.name
                            )).into());
                        }
                    },
                };

                // for partial fields put value as Value::Null, which is handled properly at encode and decode time
                value
                    .resolve_internal(&field.schema, field.index)
                    .map(|value| (field.name.clone(), value))
            }).collect::<Result<Vec<_>, _>>()?;

        Ok(Value::Record(new_fields, Self::get_value_setting(index)))
    }

    // u64 to u64 is default
    // string to u64 is through well defined patterns
    fn resolve_datetime(self, index: bool) -> Result<Self, Error> {
        match self {
            Value::Long(val, _) => Ok(Value::Date(val, Self::get_value_setting(index))),
            Value::Date(val, _) => Ok(Value::Date(val, Self::get_value_setting(index))),
            Value::String(val, _) => {
                let dt = chrono::DateTime::parse_from_rfc3339(&val);
                if !dt.is_err() {
                    let epoch = dt.unwrap().timestamp_millis();
                    return Ok(Value::Date(epoch, Self::get_value_setting(index)));
                }

                let dt = chrono::DateTime::parse_from_rfc2822(&val);
                if !dt.is_err() {
                    let epoch = dt.unwrap().timestamp_millis();
                    return Ok(Value::Date(epoch, Self::get_value_setting(index)));
                }

                return Err(failure::err_msg(format!("Couldn't resolve string value {} to date", val)));
            }
            other => Err(SchemaResolutionError::new(format!("Date expected, got {:?}", other)).into()),
        }
    }

    fn resolve_set(self, index: bool) -> Result<Self, Error> {
        match self {
            Value::Array(items, _) => Ok(Value::Set(
                items
                    .into_iter()
                    .map(|item| {
                        match item {
                            Value::String(s, _) => Ok(s),
                            other => Err(SchemaResolutionError::new(format!(
                                "String expected, got {:?}", other
                            )).into())
                        }
                    })
                    .collect::<Result<HashSet<_>, SchemaResolutionError>>()?,
                Self::get_value_setting(index),
            )),
            Value::Set(items, _) => Ok(Value::Set(items, Self::get_value_setting(index))),
            other => Err(SchemaResolutionError::new(format!(
                "Set expected, got {:?}", other
            )).into()),
        }
    }

    fn resolve_lru_value(self) -> Result<LruValue, Error> {
        let resolved = self.resolve_internal(&LRU_VALUE_SCHEMA, false)?;

        match resolved {
            Value::Record(fields, _) => {
                let mut access_time = 0;
                let mut count = 0;

                for (field, value) in fields {
                    match (field.as_str(), value) {
                        (ACCESS_TIME, Value::Long(v, _)) => {
                            access_time = v;
                        }
                        (COUNT, Value::Long(v, _)) => {
                            count = v;
                        }
                        (_, _) => {}
                    }
                }

                Ok(LruValue {
                    access_time,
                    count,
                })
            }
            other => {
                Err(SchemaResolutionError::new(format!("Expected LruValue record, got {:?}", other)).into())
            }
        }
    }

    fn resolve_lru_set(self, lru_limit: LruLimit, index: bool) -> Result<Self, Error> {
        match self {
            Value::Map(items, _) => Ok(Value::LruSet(
                items
                    .into_iter()
                    .map(|(key, value)| value.resolve_lru_value().map(|value| (key, value)))
                    .collect::<Result<HashMap<_, _>, _>>()?,
                lru_limit,
                Self::get_value_setting(index),
            )),
            Value::LruSet(items, _, _) => Ok(Value::LruSet(items, lru_limit, Self::get_value_setting(index))),
            other => Err(SchemaResolutionError::new(format!(
                "LruSet expected, got {:?}", other
            )).into()),
        }
    }

    fn resolve_optional(self, schema: &Schema, index: bool) -> Result<Self, Error> {
        let v = match self {
            // Both are Optional case.
            Value::Optional(v, _) => v,
            // Reader is an Optional, but writer is not.
            v => Some(Box::new(v)),
        };
        // Find the first match in the reader schema.
        match v {
            Some(value) => {
                let value = value.resolve(schema)?;
                Ok(Value::Optional(Some(Box::new(value)), Self::get_value_setting(index)))
            }
            None => Ok(Value::Optional(None, Self::get_value_setting(index)))
        }
    }

    pub fn json(&self) -> JsonValue {
        match self {
            Value::Null => JsonValue::Null,
            Value::Boolean(b, _) => JsonValue::Bool(b.to_owned()),
            Value::Int(n, _) => json!(n),
            Value::Long(n, _) => json!(n),
            Value::Float(n, _) => json!(n),
            Value::Double(n, _) => json!(n),
            Value::Bytes(b, _) => json!(b),
            Value::String(s, _) => JsonValue::String(s.to_owned()),
            Value::Fixed(size, data, _) => json!(data),
            Value::Enum(index, value, _) => JsonValue::String(value.to_owned()),
            Value::Union(value, _) => value.json(),
            Value::Array(items, _) => {
                JsonValue::Array(items.into_iter().map(|item| item.json()).collect::<_>())
            }
            Value::Map(items, _) => {
                JsonValue::Object(items.into_iter().map(|(key, value)| (key.clone(), value.json())).collect::<_>())
            }
            Value::Record(items, _) => {
                JsonValue::Object(items.iter().map(|(key, value)| (key.clone(), value.json())).collect::<_>())
            }
            Value::Date(t, _) => json!(t),
            Value::Set(items, _) => {
                JsonValue::Array(items.into_iter().map(|item| JsonValue::String(item.to_owned())).collect::<_>())
            }
            Value::LruSet(items, _, _) => {
                JsonValue::Object(items.into_iter().map(|(key, value)| (key.clone(), value.json())).collect::<_>())
            }
            Value::Optional(value, _) => {
                match value {
                    Some(v) => v.json(),
                    None => JsonValue::Null,
                }
            }
        }
    }

    fn try_u8(self) -> Result<u8, Error> {
        let int = self.resolve(&Schema::Int)?;
        if let Value::Int(n, None) = int {
            if n >= 0 && n <= i32::from(u8::MAX) {
                return Ok(n as u8);
            }
        }

        Err(
            SchemaResolutionError::new(
                format!("Unable to convert to u8, got {:?}", int)
            ).into()
        )
    }

    fn get_value_setting(index: bool) -> Option<ValueSetting> {
        if !index {
            None
        } else {
            Some(ValueSetting { index })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::schema::{Name, RecordField, RecordFieldOrder, UnionSchema};

    use super::*;

    #[test]
    fn validate() {
        let value_schema_valid = vec![
            (Value::Int(42, None), Schema::Int, true),
            (Value::Int(42, None), Schema::Boolean, false),
            (
                Value::Union(Box::new(Value::Null), None),
                Schema::Union(UnionSchema::new(vec![Schema::Null, Schema::Int]).unwrap()),
                true,
            ),
            (
                Value::Union(Box::new(Value::Int(42, None)), None),
                Schema::Union(UnionSchema::new(vec![Schema::Null, Schema::Int]).unwrap()),
                true,
            ),
            (
                Value::Union(Box::new(Value::Null), None),
                Schema::Union(UnionSchema::new(vec![Schema::Double, Schema::Int]).unwrap()),
                false,
            ),
            (
                Value::Union(Box::new(Value::Int(42, None)), None),
                Schema::Union(
                    UnionSchema::new(vec![
                        Schema::Null,
                        Schema::Double,
                        Schema::String,
                        Schema::Int,
                    ]).unwrap(),
                ),
                true,
            ),
            (
                Value::Array(vec![Value::Long(42i64, None)], None),
                Schema::Array(Box::new(Schema::Long)),
                true,
            ),
            (
                Value::Array(vec![Value::Boolean(true, None)], None),
                Schema::Array(Box::new(Schema::Long)),
                false,
            ),
            (Value::Record(vec![], None), Schema::Null, false),
        ];

        for (value, schema, valid) in value_schema_valid.into_iter() {
            assert_eq!(valid, value.validate(&schema));
        }
    }

    #[test]
    fn validate_fixed() {
        let schema = Schema::Fixed {
            size: 4,
            name: Name::new("some_fixed"),
        };

        assert!(Value::Fixed(4, vec![0, 0, 0, 0], None).validate(&schema));
        assert!(!Value::Fixed(5, vec![0, 0, 0, 0, 0], None).validate(&schema));
    }

    #[test]
    fn validate_enum() {
        let schema = Schema::Enum {
            name: Name::new("some_enum"),
            doc: None,
            symbols: vec![
                "spades".to_string(),
                "hearts".to_string(),
                "diamonds".to_string(),
                "clubs".to_string(),
            ],
        };

        assert!(Value::Enum(0, "spades".to_string(), None).validate(&schema));
        assert!(Value::String("spades".to_string(), None).validate(&schema));

        assert!(!Value::Enum(1, "spades".to_string(), None).validate(&schema));
        assert!(!Value::String("lorem".to_string(), None).validate(&schema));

        let other_schema = Schema::Enum {
            name: Name::new("some_other_enum"),
            doc: None,
            symbols: vec![
                "hearts".to_string(),
                "diamonds".to_string(),
                "clubs".to_string(),
                "spades".to_string(),
            ],
        };

        assert!(!Value::Enum(0, "spades".to_string(), None).validate(&other_schema));
    }

    #[test]
    fn validate_record() {
        // {
        //    "type": "record",
        //    "fields": [
        //      {"type": "long", "name": "a"},
        //      {"type": "string", "name": "b"}
        //    ]
        // }
        let schema = Schema::Record {
            name: Name::new("some_record"),
            doc: None,
            fields: vec![
                RecordField {
                    name: "a".to_string(),
                    doc: None,
                    default: None,
                    schema: Schema::Long,
                    order: RecordFieldOrder::Ascending,
                    position: 0,
                    index: false,
                },
                RecordField {
                    name: "b".to_string(),
                    doc: None,
                    default: None,
                    schema: Schema::String,
                    order: RecordFieldOrder::Ascending,
                    position: 1,
                    index: false,
                },
            ],
            lookup: HashMap::new(),
        };

        assert!(
            Value::Record(vec![
                ("a".to_string(), Value::Long(42i64, None)),
                ("b".to_string(), Value::String("foo".to_string(), None)),
            ], None).validate(&schema)
        );

        assert!(
            !Value::Record(vec![
                ("b".to_string(), Value::String("foo".to_string(), None)),
                ("a".to_string(), Value::Long(42i64, None)),
            ], None).validate(&schema)
        );

        assert!(
            !Value::Record(vec![
                ("a".to_string(), Value::Boolean(false, None)),
                ("b".to_string(), Value::String("foo".to_string(), None)),
            ], None).validate(&schema)
        );

        assert!(
            !Value::Record(vec![
                ("a".to_string(), Value::Long(42i64, None)),
                ("c".to_string(), Value::String("foo".to_string(), None)),
            ], None).validate(&schema)
        );

        assert!(
            !Value::Record(vec![
                ("a".to_string(), Value::Long(42i64, None)),
                ("b".to_string(), Value::String("foo".to_string(), None)),
                ("c".to_string(), Value::Null),
            ], None).validate(&schema)
        );
    }

    #[test]
    fn resolve_bytes_ok() {
        let value = Value::Array(vec![Value::Int(0, None), Value::Int(42, None)], None);
        assert_eq!(
            value.resolve(&Schema::Bytes).unwrap(),
            Value::Bytes(vec![0u8, 42u8], None)
        );
    }

    #[test]
    fn resolve_bytes_failure() {
        let value = Value::Array(vec![Value::Int(2000, None), Value::Int(-42, None)], None);
        assert!(value.resolve(&Schema::Bytes).is_err());
    }
}
