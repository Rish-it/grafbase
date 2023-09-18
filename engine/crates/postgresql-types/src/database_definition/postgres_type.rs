use super::{EnumId, EnumWalker};

#[derive(Clone, Copy, PartialEq)]
pub enum DatabaseType<'a> {
    Scalar(ScalarType),
    Enum(EnumWalker<'a>),
}

impl<'a> DatabaseType<'a> {
    pub fn is_enum(self) -> bool {
        matches!(self, DatabaseType::Enum(_))
    }

    pub fn is_binary(&self) -> bool {
        matches!(
            self,
            DatabaseType::Scalar(ScalarType::Bytea) | DatabaseType::Scalar(ScalarType::ByteaArray)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ColumnType {
    Scalar(ScalarType),
    Enum(EnumId),
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ScalarType {
    Bool,
    Bytea,
    Char,
    Name,
    Int8,
    Int2,
    Int2Vector,
    Int4,
    Regproc,
    Text,
    Oid,
    Tid,
    Xid,
    Cid,
    OidVector,
    PgDdlCommand,
    Json,
    Xml,
    XmlArray,
    PgNodeTree,
    JsonArray,
    TableAmHandler,
    Xid8Array,
    IndexAmHandler,
    Point,
    Lseg,
    Path,
    Box,
    Polygon,
    Line,
    LineArray,
    Cidr,
    CidrArray,
    Float4,
    Float8,
    Unknown,
    Circle,
    CircleArray,
    Macaddr8,
    Macaddr8Array,
    Money,
    MoneyArray,
    Macaddr,
    Inet,
    BoolArray,
    ByteaArray,
    CharArray,
    NameArray,
    Int2Array,
    Int2VectorArray,
    Int4Array,
    RegprocArray,
    TextArray,
    TidArray,
    XidArray,
    CidArray,
    OidVectorArray,
    BpcharArray,
    VarcharArray,
    Int8Array,
    PointArray,
    LsegArray,
    PathArray,
    BoxArray,
    Float4Array,
    Float8Array,
    PolygonArray,
    OidArray,
    Aclitem,
    AclitemArray,
    MacaddrArray,
    InetArray,
    Bpchar,
    Varchar,
    Date,
    Time,
    Timestamp,
    TimestampArray,
    DateArray,
    TimeArray,
    Timestamptz,
    TimestamptzArray,
    Interval,
    IntervalArray,
    NumericArray,
    CstringArray,
    Timetz,
    TimetzArray,
    Bit,
    BitArray,
    Varbit,
    VarbitArray,
    Numeric,
    Refcursor,
    RefcursorArray,
    Regprocedure,
    Regoper,
    Regoperator,
    Regclass,
    Regtype,
    RegprocedureArray,
    RegoperArray,
    RegoperatorArray,
    RegclassArray,
    RegtypeArray,
    Record,
    Cstring,
    Any,
    Anyarray,
    Void,
    Trigger,
    LanguageHandler,
    Internal,
    Anyelement,
    RecordArray,
    Anynonarray,
    TxidSnapshotArray,
    Uuid,
    UuidArray,
    TxidSnapshot,
    FdwHandler,
    PgLsn,
    PgLsnArray,
    TsmHandler,
    PgNdistinct,
    PgDependencies,
    Anyenum,
    TsVector,
    Tsquery,
    GtsVector,
    TsVectorArray,
    GtsVectorArray,
    TsqueryArray,
    Regconfig,
    RegconfigArray,
    Regdictionary,
    RegdictionaryArray,
    Jsonb,
    JsonbArray,
    AnyRange,
    EventTrigger,
    Int4Range,
    Int4RangeArray,
    NumRange,
    NumRangeArray,
    TsRange,
    TsRangeArray,
    TstzRange,
    TstzRangeArray,
    DateRange,
    DateRangeArray,
    Int8Range,
    Int8RangeArray,
    Jsonpath,
    JsonpathArray,
    Regnamespace,
    RegnamespaceArray,
    Regrole,
    RegroleArray,
    Regcollation,
    RegcollationArray,
    Int4multiRange,
    NummultiRange,
    TsmultiRange,
    TstzmultiRange,
    DatemultiRange,
    Int8multiRange,
    AnymultiRange,
    AnycompatiblemultiRange,
    PgBrinBloomSummary,
    PgBrinMinmaxMultiSummary,
    PgMcvList,
    PgSnapshot,
    PgSnapshotArray,
    Xid8,
    Anycompatible,
    Anycompatiblearray,
    Anycompatiblenonarray,
    AnycompatibleRange,
    Int4multiRangeArray,
    NummultiRangeArray,
    TsmultiRangeArray,
    TstzmultiRangeArray,
    DatemultiRangeArray,
    Int8multiRangeArray,
    Other(u32),
}

impl ScalarType {
    pub(crate) fn client_type(self) -> Option<&'static str> {
        use ScalarType::*;

        let type_name = match self {
            Char | Name | Text | Xml | Cidr | Macaddr8 | Macaddr | Bpchar | Varchar | Bit | Varbit | Cstring => {
                "String"
            }

            XmlArray | CidrArray | Macaddr8Array | CharArray | NameArray | TextArray | BpcharArray | VarcharArray
            | MacaddrArray | CstringArray | BitArray | VarbitArray => "[String]",

            Int8 => "BigInt",
            Oid => "UnsignedBigInt",
            Int2 | Int4 => "Int",
            Json | Jsonb => "JSON",
            JsonArray | JsonbArray => "[JSON]",
            Money | Numeric => "Decimal",
            MoneyArray | NumericArray => "[Decimal]",
            Int2Array | Int4Array => "[Int]",
            Float4Array | Float8Array => "[Float]",
            Time | Timetz => "Time",
            Int8Array => "[BigInt]",
            OidArray => "[UnsignedBigInt]",
            Float4 | Float8 => "Float",
            TimeArray | TimetzArray => "[Time]",
            Bool => "Boolean",
            Bytea => "Bytes",
            Inet => "IPAddress",
            BoolArray => "[Boolean]",
            ByteaArray => "[Bytes]",
            InetArray => "[IPAddress]",
            Date => "Date",
            Timestamp => "NaiveDateTime",
            TimestampArray => "[NaiveDateTime]",
            DateArray => "[Date]",
            Timestamptz => "DateTime",
            TimestamptzArray => "[DateTime]",
            Uuid => "Uuid",
            UuidArray => "[Uuid]",
            _ => return None,
        };

        Some(type_name)
    }
}

impl From<u32> for ScalarType {
    fn from(value: u32) -> Self {
        match value {
            16 => Self::Bool,
            17 => Self::Bytea,
            18 => Self::Char,
            19 => Self::Name,
            20 => Self::Int8,
            21 => Self::Int2,
            22 => Self::Int2Vector,
            23 => Self::Int4,
            24 => Self::Regproc,
            25 => Self::Text,
            26 => Self::Oid,
            27 => Self::Tid,
            28 => Self::Xid,
            29 => Self::Cid,
            30 => Self::OidVector,
            32 => Self::PgDdlCommand,
            114 => Self::Json,
            142 => Self::Xml,
            143 => Self::XmlArray,
            194 => Self::PgNodeTree,
            199 => Self::JsonArray,
            269 => Self::TableAmHandler,
            271 => Self::Xid8Array,
            325 => Self::IndexAmHandler,
            600 => Self::Point,
            601 => Self::Lseg,
            602 => Self::Path,
            603 => Self::Box,
            604 => Self::Polygon,
            628 => Self::Line,
            629 => Self::LineArray,
            650 => Self::Cidr,
            651 => Self::CidrArray,
            700 => Self::Float4,
            701 => Self::Float8,
            705 => Self::Unknown,
            718 => Self::Circle,
            719 => Self::CircleArray,
            774 => Self::Macaddr8,
            775 => Self::Macaddr8Array,
            790 => Self::Money,
            791 => Self::MoneyArray,
            829 => Self::Macaddr,
            869 => Self::Inet,
            1000 => Self::BoolArray,
            1001 => Self::ByteaArray,
            1002 => Self::CharArray,
            1003 => Self::NameArray,
            1005 => Self::Int2Array,
            1006 => Self::Int2VectorArray,
            1007 => Self::Int4Array,
            1008 => Self::RegprocArray,
            1009 => Self::TextArray,
            1010 => Self::TidArray,
            1011 => Self::XidArray,
            1012 => Self::CidArray,
            1013 => Self::OidVectorArray,
            1014 => Self::BpcharArray,
            1015 => Self::VarcharArray,
            1016 => Self::Int8Array,
            1017 => Self::PointArray,
            1018 => Self::LsegArray,
            1019 => Self::PathArray,
            1020 => Self::BoxArray,
            1021 => Self::Float4Array,
            1022 => Self::Float8Array,
            1027 => Self::PolygonArray,
            1028 => Self::OidArray,
            1033 => Self::Aclitem,
            1034 => Self::AclitemArray,
            1040 => Self::MacaddrArray,
            1041 => Self::InetArray,
            1042 => Self::Bpchar,
            1043 => Self::Varchar,
            1082 => Self::Date,
            1083 => Self::Time,
            1114 => Self::Timestamp,
            1115 => Self::TimestampArray,
            1182 => Self::DateArray,
            1183 => Self::TimeArray,
            1184 => Self::Timestamptz,
            1185 => Self::TimestamptzArray,
            1186 => Self::Interval,
            1187 => Self::IntervalArray,
            1231 => Self::NumericArray,
            1263 => Self::CstringArray,
            1266 => Self::Timetz,
            1270 => Self::TimetzArray,
            1560 => Self::Bit,
            1561 => Self::BitArray,
            1562 => Self::Varbit,
            1563 => Self::VarbitArray,
            1700 => Self::Numeric,
            1790 => Self::Refcursor,
            2201 => Self::RefcursorArray,
            2202 => Self::Regprocedure,
            2203 => Self::Regoper,
            2204 => Self::Regoperator,
            2205 => Self::Regclass,
            2206 => Self::Regtype,
            2207 => Self::RegprocedureArray,
            2208 => Self::RegoperArray,
            2209 => Self::RegoperatorArray,
            2210 => Self::RegclassArray,
            2211 => Self::RegtypeArray,
            2249 => Self::Record,
            2275 => Self::Cstring,
            2276 => Self::Any,
            2277 => Self::Anyarray,
            2278 => Self::Void,
            2279 => Self::Trigger,
            2280 => Self::LanguageHandler,
            2281 => Self::Internal,
            2283 => Self::Anyelement,
            2287 => Self::RecordArray,
            2776 => Self::Anynonarray,
            2949 => Self::TxidSnapshotArray,
            2950 => Self::Uuid,
            2951 => Self::UuidArray,
            2970 => Self::TxidSnapshot,
            3115 => Self::FdwHandler,
            3220 => Self::PgLsn,
            3221 => Self::PgLsnArray,
            3310 => Self::TsmHandler,
            3361 => Self::PgNdistinct,
            3402 => Self::PgDependencies,
            3500 => Self::Anyenum,
            3614 => Self::TsVector,
            3615 => Self::Tsquery,
            3642 => Self::GtsVector,
            3643 => Self::TsVectorArray,
            3644 => Self::GtsVectorArray,
            3645 => Self::TsqueryArray,
            3734 => Self::Regconfig,
            3735 => Self::RegconfigArray,
            3769 => Self::Regdictionary,
            3770 => Self::RegdictionaryArray,
            3802 => Self::Jsonb,
            3807 => Self::JsonbArray,
            3831 => Self::AnyRange,
            3838 => Self::EventTrigger,
            3904 => Self::Int4Range,
            3905 => Self::Int4RangeArray,
            3906 => Self::NumRange,
            3907 => Self::NumRangeArray,
            3908 => Self::TsRange,
            3909 => Self::TsRangeArray,
            3910 => Self::TstzRange,
            3911 => Self::TstzRangeArray,
            3912 => Self::DateRange,
            3913 => Self::DateRangeArray,
            3926 => Self::Int8Range,
            3927 => Self::Int8RangeArray,
            4072 => Self::Jsonpath,
            4073 => Self::JsonpathArray,
            4989 => Self::Regnamespace,
            4090 => Self::RegnamespaceArray,
            4096 => Self::Regrole,
            4097 => Self::RegroleArray,
            4191 => Self::Regcollation,
            4192 => Self::RegcollationArray,
            4451 => Self::Int4multiRange,
            4532 => Self::NummultiRange,
            4533 => Self::TsmultiRange,
            4534 => Self::TstzmultiRange,
            4535 => Self::DatemultiRange,
            4536 => Self::Int8multiRange,
            4537 => Self::AnymultiRange,
            4538 => Self::AnycompatiblemultiRange,
            4600 => Self::PgBrinBloomSummary,
            4601 => Self::PgBrinMinmaxMultiSummary,
            5017 => Self::PgMcvList,
            5038 => Self::PgSnapshot,
            5039 => Self::PgSnapshotArray,
            5069 => Self::Xid8,
            5077 => Self::Anycompatible,
            5078 => Self::Anycompatiblearray,
            5079 => Self::Anycompatiblenonarray,
            5080 => Self::AnycompatibleRange,
            6150 => Self::Int4multiRangeArray,
            6151 => Self::NummultiRangeArray,
            6152 => Self::TsmultiRangeArray,
            6153 => Self::TstzmultiRangeArray,
            6155 => Self::DatemultiRangeArray,
            6157 => Self::Int8multiRangeArray,
            _ => Self::Other(value),
        }
    }
}
