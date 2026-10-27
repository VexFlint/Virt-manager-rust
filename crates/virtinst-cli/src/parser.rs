//! Upstream's `VirtCLIParser` machinery from `cli.py`: how a compound
//! option like `--disk path=/foo,bus=virtio` is split
//! (`parse_optstr_tuples`, `_parse_optstr_to_dict`), matched against a
//! parser's registered sub-options (`_VirtCLIArgumentStatic`), and
//! applied to an [`XmlObject`] (`_VirtCLIArgument.parse_param`), or
//! used to pick existing objects for `virt-xml --edit`
//! (`lookup_child_from_option_string`).
//!
//! The parsers themselves (`ParserWatchdog`, ...) live in
//! [`crate::parsers`], one [`ParserDef`] each, built the way upstream's
//! `_virtcli_class_init` calls `add_arg`.

use virtinst_core::xmlbuilder::{Value, XmlClass, XmlManualAction, XmlObject};

/// An error as `fail()` would print it.
pub type Error = String;

/// `parse_optstr_tuples`'s tokenizer: `shlex.shlex(optstr, posix=True)`
/// with `,` as the only whitespace, `whitespace_split` on and no
/// comments, so `path="/a,b"` and `path=/a\,b` keep their comma.
pub fn shlex_split_commas(optstr: &str) -> Vec<String> {
    let mut out = vec![];
    let mut token = String::new();
    // A quoted empty string still makes a token.
    let mut have_token = false;
    let mut chars = optstr.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ',' => {
                if have_token {
                    out.push(std::mem::take(&mut token));
                    have_token = false;
                }
            }
            '\\' => {
                have_token = true;
                match chars.next() {
                    Some(n) => token.push(n),
                    None => token.push('\\'),
                }
            }
            '\'' => {
                have_token = true;
                for n in chars.by_ref() {
                    if n == '\'' {
                        break;
                    }
                    token.push(n);
                }
            }
            '"' => {
                have_token = true;
                while let Some(n) = chars.next() {
                    match n {
                        '"' => break,
                        '\\' if matches!(chars.peek(), Some('"' | '\\')) => {
                            token.push(chars.next().unwrap());
                        }
                        _ => token.push(n),
                    }
                }
            }
            _ => {
                have_token = true;
                token.push(c);
            }
        }
    }
    if have_token {
        out.push(token);
    }
    out
}

/// `parse_optstr_tuples`: `path=foo,size=5,path=bar` becomes
/// `[("path", "foo"), ("size", "5"), ("path", "bar")]`, a bare `foo`
/// becomes `("foo", None)`.
pub fn parse_optstr_tuples(optstr: &str) -> Vec<(String, Option<String>)> {
    shlex_split_commas(optstr)
        .into_iter()
        .map(|opt| match opt.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (opt, None),
        })
        .collect()
}

/// A callback setting a sub-option (`cb=`): parser state, the object,
/// the converted value, and the argument (for its key and propname).
pub type SetCb = fn(&mut ParseCtx, &mut XmlObject, Option<Value>, &ArgInst) -> Result<(), Error>;

/// A `lookup_cb=`: whether the object matches the value.
pub type LookupCb = fn(&ParseCtx, &XmlObject, Option<Value>, &ArgInst) -> Result<bool, Error>;

/// `_make_find_inst_cb`: sub-options like `seclabel1.model` act on the
/// Nth object of a child list (`seclabels`), created on demand.
#[derive(Clone, Copy, Debug)]
pub struct FindInst {
    /// The option name the index follows, `seclabel`.
    pub cliarg: &'static str,
    /// The child list, through single children (`source.seclabels`).
    pub list_propname: &'static str,
}

/// `_VirtCLIArgumentStatic`.
#[derive(Clone)]
pub struct ArgDef {
    pub cliname: &'static str,
    pub propname: Option<&'static str>,
    pub cb: Option<SetCb>,
    pub can_comma: bool,
    pub ignore_default: bool,
    pub is_onoff: bool,
    pub lookup_cb: Option<LookupCb>,
    /// `lookup_cb=None` given explicitly: no lookup, not an error.
    pub no_lookup: bool,
    pub find_inst: Option<FindInst>,
    /// A second `find_inst_cb` level inside the first one's object
    /// (`portForward[0-9]*.range[0-9]*`).
    pub find_inst_nested: Option<FindInst>,
    pub aliases: Vec<&'static str>,
}

impl ArgDef {
    /// `add_arg(cliname, propname)`.
    pub fn new(cliname: &'static str, propname: &'static str) -> ArgDef {
        ArgDef {
            cliname,
            propname: Some(propname),
            cb: None,
            can_comma: false,
            ignore_default: false,
            is_onoff: false,
            lookup_cb: None,
            no_lookup: false,
            find_inst: None,
            find_inst_nested: None,
            aliases: vec![],
        }
    }

    /// `add_arg(cliname, None, cb=..., lookup_cb=None)`.
    pub fn cb_only(cliname: &'static str, cb: SetCb) -> ArgDef {
        ArgDef {
            propname: None,
            cb: Some(cb),
            no_lookup: true,
            ..ArgDef::new(cliname, "")
        }
    }

    pub fn cb(mut self, cb: SetCb) -> ArgDef {
        self.cb = Some(cb);
        self
    }

    pub fn can_comma(mut self) -> ArgDef {
        self.can_comma = true;
        self
    }

    pub fn ignore_default(mut self) -> ArgDef {
        self.ignore_default = true;
        self
    }

    pub fn onoff(mut self) -> ArgDef {
        self.is_onoff = true;
        self
    }

    pub fn lookup(mut self, cb: LookupCb) -> ArgDef {
        self.lookup_cb = Some(cb);
        self
    }

    pub fn no_lookup(mut self) -> ArgDef {
        self.no_lookup = true;
        self
    }

    pub fn find_inst(mut self, cliarg: &'static str, list_propname: &'static str) -> ArgDef {
        self.find_inst = Some(FindInst {
            cliarg,
            list_propname,
        });
        self
    }

    /// A `find_inst_cb` that first finds the outer object, then the
    /// inner list's object in it.
    pub fn find_inst_in(
        mut self,
        outer: (&'static str, &'static str),
        inner: (&'static str, &'static str),
    ) -> ArgDef {
        self = self.find_inst(outer.0, outer.1);
        self.find_inst_nested = Some(FindInst {
            cliarg: inner.0,
            list_propname: inner.1,
        });
        self
    }

    /// `match_name`: the cliname or an alias, where `[0-9]*` matches any
    /// run of digits.
    pub fn match_name(&self, userstr: &str) -> bool {
        std::iter::once(&self.cliname)
            .chain(self.aliases.iter())
            .any(|c| pattern_matches(c, userstr))
    }
}

/// `re.match("^%s$" % cliname.replace(".", r"\."), userstr)` for the
/// only regex construct clinames use, `[0-9]*`.
fn pattern_matches(pattern: &str, s: &str) -> bool {
    match pattern.split_once("[0-9]*") {
        None => pattern == s,
        Some((pre, post)) => {
            let Some(rest) = s.strip_prefix(pre) else {
                return false;
            };
            let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
            // `[0-9]*` is greedy but may backtrack: try every split.
            (0..=digits).any(|n| pattern_matches(post, &rest[n..]))
        }
    }
}

/// One parser's custom `_parse` pre-processing of the option dict.
pub type PreParse = fn(&mut OptDict) -> Result<(), Error>;

/// A parser's own `_parse` override, run before the generic one:
/// `Some(objects)` replaces the generic parse with those (possibly no)
/// objects, `None` falls through to it.
pub type CustomParse = fn(&mut ParseCtx) -> Result<Option<Vec<XmlObject>>, Error>;

/// The `conn.support` answers parsers read.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConnSupport {
    /// `conn_qemu_xhci`.
    pub qemu_xhci: bool,
    /// `conn_usb3_ports`.
    pub usb3_ports: bool,
    /// `conn.is_xen()`.
    pub is_xen: bool,
    /// `conn.is_test()`.
    pub is_test: bool,
    /// `conn.is_qemu_unprivileged()`.
    pub is_qemu_unprivileged: bool,
    /// `conn.is_remote()`.
    pub is_remote: bool,
    /// `conn_vmport`.
    pub vmport: bool,
    /// `conn_graphics_listen_none`.
    pub graphics_listen_none: bool,
}

thread_local! {
    static CONN_SUPPORT: std::cell::Cell<ConnSupport> =
        const {
            std::cell::Cell::new(ConnSupport {
                qemu_xhci: false,
                usb3_ports: true,
                is_xen: false,
                is_test: false,
                is_qemu_unprivileged: false,
                is_remote: false,
                vmport: false,
                graphics_listen_none: true,
            })
        };
}

thread_local! {
    static NODEDEVS: std::cell::RefCell<Vec<virtinst_core::nodedev::NodeDevice>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Record `conn.fetch_all_nodedevs()` for the parsers that look node
/// devices up (`--hostdev`).
pub fn set_nodedevs(devs: Vec<virtinst_core::nodedev::NodeDevice>) {
    NODEDEVS.with(|n| *n.borrow_mut() = devs);
}

/// `conn.fetch_all_nodedevs()`, as recorded.
pub fn nodedevs() -> Vec<virtinst_core::nodedev::NodeDevice> {
    NODEDEVS.with(|n| n.borrow().clone())
}

/// Record the connection's support answers for the parsers
/// (`guest.conn.support`).
pub fn set_conn_support(support: ConnSupport) {
    CONN_SUPPORT.with(|c| c.set(support));
}

/// The connection's support answers.
pub fn conn_support() -> ConnSupport {
    CONN_SUPPORT.with(|c| c.get())
}

/// A `VirtCLIParser` subclass.
pub struct ParserDef {
    pub cli_arg_name: &'static str,
    /// `guest_propname` under the guest, `devices.watchdog`.
    pub guest_propname: Option<&'static str>,
    pub remove_first: &'static [&'static str],
    pub stub_none: bool,
    pub supports_clearxml: bool,
    pub args: Vec<ArgDef>,
    pub pre_parse: Option<PreParse>,
    pub custom_parse: Option<CustomParse>,
    /// The class of a new object, for list `guest_propname`s.
    pub class: Option<&'static XmlClass>,
}

impl ParserDef {
    /// A parser with `_virtcli_class_init_common` done: `clearxml` first
    /// and, for guest backed parsers, the `xpath[0-9]*.*` arguments.
    pub fn new(
        cli_arg_name: &'static str,
        guest_propname: Option<&'static str>,
        class: Option<&'static XmlClass>,
    ) -> ParserDef {
        let mut def = ParserDef {
            cli_arg_name,
            guest_propname,
            remove_first: &[],
            stub_none: true,
            supports_clearxml: true,
            args: vec![],
            pre_parse: None,
            custom_parse: None,
            class,
        };
        def.args
            .push(ArgDef::cb_only("clearxml", clearxml_cb).onoff().no_lookup());
        if guest_propname.is_some() {
            def.add_xpath_args();
        }
        def
    }

    /// `_add_xpath_args`: the `xpath[0-9]*.*` arguments.
    fn add_xpath_args(&mut self) {
        for (cli, prop) in [
            ("xpath[0-9]*.delete", "xpath_delete"),
            ("xpath[0-9]*.set", "xpath_set"),
            ("xpath[0-9]*.create", "xpath_create"),
            ("xpath[0-9]*.value", "xpath_value"),
        ] {
            self.args.push(ArgDef {
                can_comma: true,
                no_lookup: true,
                ..ArgDef::new(cli, prop).find_inst("xpath", "xml_actions")
            });
        }
    }

    /// `add_arg`.
    pub fn arg(&mut self, arg: ArgDef) {
        self.args.push(arg);
    }

    /// `cls.aliases[cliname] = ...` for an already added argument.
    pub fn alias(&mut self, cliname: &str, aliases: &[&'static str]) {
        let arg = self
            .args
            .iter_mut()
            .find(|a| a.cliname == cliname)
            .unwrap_or_else(|| panic!("no argument {cliname}"));
        arg.aliases.extend_from_slice(aliases);
    }

    /// `cli_flag_name`.
    pub fn cli_flag_name(&self) -> String {
        format!("--{}", self.cli_arg_name.replace('_', "-"))
    }

    /// `print_introspection`: `--disk=?`/`--disk help`.
    pub fn introspection(&self) -> String {
        let key = |a: &&ArgDef| {
            let prefix = if a.cliname == "clearxml" {
                "0"
            } else if a.cliname.starts_with("address.") {
                "1"
            } else if a.cliname.starts_with("xpath") {
                "2"
            } else {
                ""
            };
            format!("{prefix}{}", a.cliname)
        };
        let mut args: Vec<&ArgDef> = self.args.iter().collect();
        args.sort_by_key(key);
        let mut out = format!("{} options:\n", self.cli_flag_name());
        for a in args {
            out += &format!("  {}\n", a.cliname);
        }
        out.push('\n');
        out
    }

    fn lookup_arg(&self, cliname: &str) -> Option<&ArgDef> {
        self.args.iter().find(|a| a.match_name(cliname))
    }

    /// `_parse_optstr_to_dict`.
    pub fn optdict(&self, optstr: &str) -> OptDict {
        let mut tuples = parse_optstr_tuples(optstr);
        let mut remove_first: Vec<&str> = self.remove_first.to_vec();
        // Splice in remove_first names upfront
        for t in tuples.iter_mut() {
            if t.1.is_some() || remove_first.is_empty() {
                break;
            }
            let name = remove_first.remove(0);
            t.1 = Some(std::mem::replace(&mut t.0, name.to_string()));
        }
        let mut dict = OptDict::default();
        let mut tuples: std::collections::VecDeque<_> = tuples.into();
        while let Some((cliname, val)) = tuples.pop_front() {
            let Some(arg) = self.lookup_arg(&cliname) else {
                dict.insert(cliname, val);
                continue;
            };
            let mut val = val;
            if arg.can_comma {
                // `_consume_comma_arg`
                let mut joined = val.unwrap_or_default();
                while let Some((next, nval)) = tuples.front() {
                    if self.lookup_arg(next).is_some() {
                        break;
                    }
                    joined += &format!(",{next}");
                    if let Some(v) = nval {
                        if !v.is_empty() {
                            joined += &format!("={v}");
                        }
                    }
                    tuples.pop_front();
                }
                val = Some(joined);
            }
            dict.insert(cliname, val);
        }
        dict
    }
}

/// `collections.OrderedDict` of `cliname -> value`: a repeated key keeps
/// its first position and its last value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OptDict(pub Vec<(String, Option<String>)>);

impl OptDict {
    pub fn insert(&mut self, key: String, val: Option<String>) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => e.1 = val,
            None => self.0.push((key, val)),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Option<String>> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn remove(&mut self, key: &str) -> Option<Option<String>> {
        let i = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(i).1)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn keys(&self) -> Vec<String> {
        self.0.iter().map(|(k, _)| k.clone()).collect()
    }
}

/// `_VirtCLIArgument`: one `key=val` from the command line, matched to
/// its [`ArgDef`].
#[derive(Clone)]
pub struct ArgInst {
    pub key: String,
    pub val: Option<Value>,
    pub def: ArgDef,
}

impl ArgInst {
    pub fn propname(&self) -> Option<&'static str> {
        self.def.propname
    }
}

/// Why converting a sub-option value failed: upstream raises some
/// (wrapped as "Error: --flag ...") and `fail()`s on others directly.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// An exception, reported as `Error: --flag optstr: err`.
    Raised(Error),
    /// A direct `fail()`, reported as is.
    Fail(Error),
}

/// `_on_off_convert`.
fn on_off_convert(key: &str, val: &str) -> Result<bool, ParseError> {
    crate::cli::raw_on_off_convert(val)
        .ok_or_else(|| ParseError::Fail(format!("{key} must be 'yes' or 'no'")))
}

impl ArgInst {
    /// `_VirtCLIArgument.__init__`.
    fn new(def: &ArgDef, key: String, val: Option<String>) -> Result<ArgInst, ParseError> {
        let Some(val) = val else {
            // When a command line tuple option has no value set, say
            //   --network bridge=br0,model=virtio
            // is instead called
            //   --network bridge=br0,model
            // We error that 'model' didn't have a value
            return Err(ParseError::Raised(format!(
                "Option '{key}' had no value set."
            )));
        };
        let val = if val.is_empty() {
            None
        } else if def.is_onoff {
            Some(Value::Bool(on_off_convert(&key, &val)?))
        } else {
            Some(Value::Str(val))
        };
        Ok(ArgInst {
            key,
            val,
            def: def.clone(),
        })
    }
}

/// What a parser works against: `self.guest`, `self.optstr`,
/// `self.editing` and the parsed option dict.
pub struct ParseCtx<'a> {
    pub def: &'a ParserDef,
    pub guest: &'a mut XmlObject,
    pub optstr: String,
    pub optdict: OptDict,
    pub editing: bool,
    /// A `boot.order=` value, applied with `guest.reorder_boot_order`
    /// once the object is in the guest.
    pub boot_order: Option<i64>,
    /// Guest side effects of an edit, run on the guest once the edited
    /// object is back in it (e.g. `guest.change_graphics`).
    pub post_edit: Option<PostEdit>,
}

/// See `ParseCtx::post_edit`: the guest and the edited object's index.
pub type PostEdit = fn(&mut XmlObject, usize) -> Result<(), Error>;

/// `_clearxml_cb`.
fn clearxml_cb(
    ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    if ctx.def.guest_propname.is_none() {
        return Err(format!(
            "Don't know how to clearxml for {}",
            ctx.def.cli_flag_name()
        ));
    }
    if val != Some(Value::Bool(true)) {
        return Ok(());
    }
    // If there's any opts remaining, leave the root stub element
    // in place with leave_stub=True, so virt-xml updates are done
    // in place.
    inst.clear(ctx.optstr.contains(','));
    Ok(())
}

/// The child list a [`FindInst`] names, through single children.
fn child_list<'o>(inst: &'o mut XmlObject, list_propname: &str) -> &'o mut Vec<XmlObject> {
    match list_propname.split_once('.') {
        Some((c, rest)) => child_list(inst.child_mut(c), rest),
        None => inst.children_mut(list_propname),
    }
}

/// The parent of the child list a [`FindInst`] names.
fn child_list_parent<'o>(
    inst: &'o mut XmlObject,
    list_propname: &str,
) -> (&'o mut XmlObject, String) {
    match list_propname.rsplit_once('.') {
        Some((parents, last)) => {
            let mut obj = inst;
            for c in parents.split('.') {
                obj = obj.child_mut(c);
            }
            (obj, last.to_string())
        }
        None => (inst, list_propname.to_string()),
    }
}

/// `_make_find_inst_cb`'s index: the digits after `cliarg` in the key.
fn find_inst_num(fi: &FindInst, key: &str) -> usize {
    let Some(i) = key.find(fi.cliarg) else {
        return 0;
    };
    let rest = &key[i + fi.cliarg.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

/// `_make_find_inst_cb(...)(can_edit=True)`: the Nth object of the
/// list, created as needed.
fn find_inst_edit<'o>(inst: &'o mut XmlObject, fi: &FindInst, key: &str) -> &'o mut XmlObject {
    let num = find_inst_num(fi, key);
    let (parent, list) = child_list_parent(inst, fi.list_propname);
    while parent.children(&list).len() < num + 1 {
        let class = parent
            .class
            .child(&list)
            .expect("find_inst list is a child property")
            .class;
        parent.add_child(XmlObject::new(class));
    }
    &mut child_list(inst, fi.list_propname)[num]
}

/// `_make_find_inst_cb(...)(can_edit=False)`: the Nth object of the
/// list, if there is one.
fn find_inst_lookup<'o>(inst: &'o XmlObject, fi: &FindInst, key: &str) -> Option<&'o XmlObject> {
    let num = find_inst_num(fi, key);
    let mut obj = inst;
    let mut parts = fi.list_propname.split('.').peekable();
    while let Some(p) = parts.next() {
        if parts.peek().is_some() {
            obj = obj.child(p);
        } else {
            return obj.children(p).get(num);
        }
    }
    None
}

impl<'a> ParseCtx<'a> {
    /// `_optdict_to_param_list`: argument order is registration order,
    /// each with every key it matches.
    fn param_list(&self, optdict: &mut OptDict) -> Result<Vec<ArgInst>, ParseError> {
        let mut ret = vec![];
        for def in &self.def.args {
            for key in optdict.keys() {
                if def.match_name(&key) {
                    let val = optdict.remove(&key).flatten();
                    ret.push(ArgInst::new(def, key, val)?);
                }
            }
        }
        Ok(ret)
    }

    /// `_check_leftover_opts`.
    fn check_leftover(&self, optdict: &OptDict) -> Result<(), ParseError> {
        if optdict.is_empty() {
            return Ok(());
        }
        let keys: Vec<String> = optdict.keys().iter().map(|k| format!("'{k}'")).collect();
        Err(ParseError::Fail(format!(
            "Unknown {} options: [{}]",
            self.def.cli_flag_name(),
            keys.join(", ")
        )))
    }

    /// `parse_param`.
    fn parse_param(&mut self, inst: &mut XmlObject, arg: &ArgInst) -> Result<(), Error> {
        if arg.def.ignore_default && arg.val == Some(Value::Str("default".into())) {
            return Ok(());
        }
        if let Some(fi) = arg.def.find_inst {
            if fi.list_propname == "xml_actions" {
                let num = find_inst_num(&fi, &arg.key);
                while inst.xml_actions.len() < num + 1 {
                    inst.xml_actions.push(XmlManualAction::default());
                }
                let prop = arg.def.propname.expect("xpath args have a propname");
                inst.xml_actions[num].set(prop, arg.val.as_ref().map(|v| v.to_xml_string()));
                return Ok(());
            }
            let mut target = find_inst_edit(inst, &fi, &arg.key);
            if let Some(nested) = arg.def.find_inst_nested {
                target = find_inst_edit(target, &nested, &arg.key);
            }
            return self.set_on(target, arg);
        }
        self.set_on(inst, arg)
    }

    fn set_on(&mut self, inst: &mut XmlObject, arg: &ArgInst) -> Result<(), Error> {
        if let Some(cb) = arg.def.cb {
            return cb(self, inst, arg.val.clone(), arg);
        }
        let prop = arg
            .def
            .propname
            .expect("an argument has a propname or a cb");
        inst.set_path(prop, arg.val.clone())
    }

    /// `lookup_param`.
    fn lookup_param(&self, inst: &XmlObject, arg: &ArgInst) -> Result<bool, Error> {
        if arg.def.propname.is_none() && arg.def.lookup_cb.is_none() {
            return Err(format!(
                "Don't know how to match device type '{}' property '{}'",
                inst.class.xml_name, arg.key
            ));
        }
        let mut inst = inst;
        for fi in [arg.def.find_inst, arg.def.find_inst_nested]
            .into_iter()
            .flatten()
        {
            match find_inst_lookup(inst, &fi, &arg.key) {
                Some(o) => inst = o,
                None => return Ok(false),
            }
        }
        if let Some(cb) = arg.def.lookup_cb {
            return cb(self, inst, arg.val.clone(), arg);
        }
        let prop = arg.def.propname.expect("checked above");
        let xmlval = inst.get_path(prop);
        let clival = inst.normalize_path(prop, arg.val.clone())?;
        Ok(xmlval == clival)
    }
}

/// `fail()`'s message for an exception inside a parser.
fn raised(def: &ParserDef, optstr: &str, err: &str) -> Error {
    format!("Error: {} {optstr}: {err}", def.cli_flag_name())
}

fn to_fail(def: &ParserDef, optstr: &str, e: ParseError) -> Error {
    match e {
        ParseError::Raised(e) => raised(def, optstr, &e),
        ParseError::Fail(e) => e,
    }
}

/// `_parse`: every matched argument applied to `inst`, then any
/// unknown option fails.
fn parse_into(ctx: &mut ParseCtx, inst: &mut XmlObject) -> Result<(), ParseError> {
    let mut optdict = ctx.optdict.clone();
    if let Some(pre) = ctx.def.pre_parse {
        pre(&mut optdict).map_err(ParseError::Raised)?;
    }
    for arg in ctx.param_list(&mut optdict)? {
        ctx.parse_param(inst, &arg).map_err(ParseError::Raised)?;
    }
    ctx.check_leftover(&optdict)?;
    for action in &inst.xml_actions {
        action.validate().map_err(ParseError::Raised)?;
    }
    Ok(())
}

/// Where a parser's objects live under the guest: the list name and
/// the parents leading to it (`devices.watchdog`).
fn guest_list<'g>(
    guest: &'g mut XmlObject,
    propname: &str,
) -> (&'g mut XmlObject, &'static str, bool) {
    let (parents, last) = match propname.rsplit_once('.') {
        Some((p, l)) => (Some(p), l),
        None => (None, propname),
    };
    let mut obj = guest;
    if let Some(parents) = parents {
        for c in parents.split('.') {
            obj = obj.child_mut(c);
        }
    }
    let cp = obj
        .class
        .child(last)
        .unwrap_or_else(|| panic!("guest has no {propname}"));
    (obj, cp.name, cp.is_single)
}

/// The objects a parser's `guest_propname` currently has.
pub fn lookup_prop<'g>(guest: &'g mut XmlObject, def: &ParserDef) -> &'g mut Vec<XmlObject> {
    let (parent, name, _) = guest_list(guest, def.guest_propname.expect("guest backed parser"));
    parent.children_mut(name)
}

/// `run_parser(guest, parserclass, optstr)` with no edit target: parse
/// into a new object added to the guest (or the guest's single object).
/// Returns where the touched objects are, as indexes into the list.
pub fn parse_new(
    def: &ParserDef,
    guest: &mut XmlObject,
    optstr: &str,
) -> Result<Vec<usize>, Error> {
    if optstr.is_empty() {
        return Ok(vec![]);
    }
    if def.stub_none && optstr == "none" {
        return Ok(vec![]);
    }
    let propname = def
        .guest_propname
        .expect("only guest backed parsers create objects");
    let (_, _, is_single) = guest_list(guest, propname);
    let optdict = def.optdict(optstr);
    if is_single {
        // Edit the guest's one object in place, taken out of the guest
        // while the parser holds the guest too.
        let mut taken = {
            let (parent, name, _) = guest_list(guest, propname);
            std::mem::take(parent.children_mut(name))
        };
        let r = {
            let mut ctx = ParseCtx {
                def,
                guest,
                optstr: optstr.to_string(),
                optdict,
                editing: false,
                boot_order: None,
                post_edit: None,
            };
            parse_into(&mut ctx, &mut taken[0])
        };
        let (parent, name, _) = guest_list(guest, propname);
        *parent.children_mut(name) = taken;
        r.map_err(|e| to_fail(def, optstr, e))?;
        return Ok(vec![0]);
    }
    let mut obj = XmlObject::new(def.class.expect("list parsers have a class"));
    let boot_order;
    {
        let mut ctx = ParseCtx {
            def,
            guest,
            optstr: optstr.to_string(),
            optdict,
            editing: false,
            boot_order: None,
            post_edit: None,
        };
        if let Some(custom) = def.custom_parse {
            if let Some(objs) = custom(&mut ctx).map_err(|e| raised(def, optstr, &e))? {
                let mut idxs = vec![];
                for obj in objs {
                    let (parent, _, _) = guest_list(guest, propname);
                    parent.add_child(obj);
                    idxs.push(lookup_prop(guest, def).len() - 1);
                }
                return Ok(idxs);
            }
        }
        parse_into(&mut ctx, &mut obj).map_err(|e| to_fail(def, optstr, e))?;
        boot_order = ctx.boot_order;
    }
    if let Some(order) = boot_order {
        reorder_boot_order(guest, BootTarget::New(&mut obj), order);
    }
    let (parent, _, _) = guest_list(guest, propname);
    parent.add_child(obj);
    Ok(vec![lookup_prop(guest, def).len() - 1])
}

/// `run_parser(..., editinst=...)`: parse into the `idx`th object of
/// the parser's list.
pub fn parse_edit(
    def: &ParserDef,
    guest: &mut XmlObject,
    idx: usize,
    optstr: &str,
) -> Result<(), Error> {
    if optstr.is_empty() || (def.stub_none && optstr == "none") {
        return Ok(());
    }
    let propname = def.guest_propname.expect("guest backed parser");
    let mut optdict = def.optdict(optstr);
    if let Some(custom) = def.custom_parse {
        let mut ctx = ParseCtx {
            def,
            guest,
            optstr: optstr.to_string(),
            optdict,
            editing: true,
            boot_order: None,
            post_edit: None,
        };
        // Objects a custom parse makes aren't added when editing.
        if custom(&mut ctx)
            .map_err(|e| raised(def, optstr, &e))?
            .is_some()
        {
            return Ok(());
        }
        // What the custom parse left of the options.
        optdict = ctx.optdict;
    }
    let mut taken = {
        let (parent, name, _) = guest_list(guest, propname);
        std::mem::take(parent.children_mut(name))
    };
    let r = {
        let mut ctx = ParseCtx {
            def,
            guest,
            optstr: optstr.to_string(),
            optdict,
            editing: true,
            boot_order: None,
            post_edit: None,
        };
        let r = parse_into(&mut ctx, &mut taken[idx]);
        (r, ctx.boot_order, ctx.post_edit)
    };
    let (parent, name, _) = guest_list(guest, propname);
    *parent.children_mut(name) = taken;
    let (r, boot_order, post_edit) = r;
    r.map_err(|e| to_fail(def, optstr, e))?;
    if let Some(order) = boot_order {
        reorder_boot_order(guest, BootTarget::Existing(name, idx), order);
    }
    if let Some(hook) = post_edit {
        hook(guest, idx).map_err(|e| raised(def, optstr, &e))?;
    }
    Ok(())
}

/// `Guest.get_bootable_devices`: the device lists that can boot.
const BOOTABLE_DEVICES: [&str; 4] = ["disk", "interface", "hostdev", "redirdev"];

/// The device `reorder_boot_order` gives the boot index: a new one not
/// in the guest yet, or the `idx`th of a guest device list.
enum BootTarget<'o> {
    New(&'o mut XmlObject),
    Existing(&'static str, usize),
}

/// `Guest.reorder_boot_order`: give the target device boot index
/// `boot_index`, shifting the others' up to the first hole, and drop
/// the `<os><boot dev=.../>` order (the two are exclusive).
fn reorder_boot_order(guest: &mut XmlObject, target: BootTarget, boot_index: i64) {
    // unset legacy boot order
    guest.with_doc_unsynced(|doc, root| {
        if let Some(os) = root.find(doc, "os") {
            for boot in os.find_all(doc, "boot") {
                let _ = boot.detach(doc);
            }
        }
    });
    let devices = guest.child_mut("devices");
    let order = |d: &XmlObject| match d.get_path("boot.order") {
        Some(Value::Int(n)) => Some(n),
        _ => None,
    };
    // Sort the bootable devices by boot order
    let mut sorted: Vec<(&str, usize, i64)> = vec![];
    for name in BOOTABLE_DEVICES {
        for (i, d) in devices.children(name).iter().enumerate() {
            if let Some(n) = order(d) {
                sorted.push((name, i, n));
            }
        }
    }
    sorted.sort_by_key(|(_, _, n)| *n);
    // set new boot order
    let set = |devices: &mut XmlObject, name: &str, i: usize, n: i64| {
        let _ = devices.children_mut(name)[i].set_path("boot.order", Some(Value::Int(n)));
    };
    let skip = match target {
        BootTarget::New(dev) => {
            let _ = dev.set_path("boot.order", Some(Value::Int(boot_index)));
            None
        }
        BootTarget::Existing(list, idx) => {
            set(devices, list, idx, boot_index);
            Some((list, idx))
        }
    };
    let mut next_boot_index = None;
    for (name, i, n) in sorted {
        if skip == Some((name, i)) {
            continue;
        }
        if Some(n) == next_boot_index || n == boot_index {
            let next = n + 1;
            next_boot_index = Some(next);
            set(devices, name, i, next);
            continue;
        }
        if next_boot_index.is_some() {
            // we found a hole so we can stop here
            break;
        }
    }
}

/// `set_boot_order_cb`: `boot.order` is applied once the object is in
/// the guest.
fn set_boot_order_cb(
    ctx: &mut ParseCtx,
    inst: &mut XmlObject,
    val: Option<Value>,
    _arg: &ArgInst,
) -> Result<(), Error> {
    let val = val.map(|v| v.to_xml_string()).unwrap_or_default();
    let n = val
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("invalid literal for int() with base 10: '{val}'"))?;
    ctx.boot_order = Some(n);
    if !ctx.editing {
        // Upstream sets `dev.boot.order` right here, so a new device's
        // `order` comes before the attributes set after it.
        inst.set_path("boot.order", Some(Value::Int(n)))?;
    }
    Ok(())
}

/// `_add_common_device_args(boot_order=True)`'s `boot.order`, added
/// right after the common arguments.
pub fn add_boot_order_arg(def: &mut ParserDef) {
    def.arg(ArgDef::new("boot.order", "boot.order").cb(set_boot_order_cb));
    def.alias("boot.order", &["boot_order"]);
}

/// `lookup_child_from_option_string`: the indexes of the objects every
/// option of `optstr` matches.
pub fn lookup_children(
    def: &ParserDef,
    guest: &mut XmlObject,
    optstr: &str,
) -> Result<Vec<usize>, Error> {
    let optdict = def.optdict(optstr);
    let objs = std::mem::take(lookup_prop(guest, def));
    let mut stand_in = XmlObject::new(&EMPTY);
    let r = (|| {
        let ctx = ParseCtx {
            def,
            guest: &mut stand_in,
            optstr: optstr.to_string(),
            optdict: optdict.clone(),
            editing: true,
            boot_order: None,
            post_edit: None,
        };
        let mut ret = vec![];
        for (i, inst) in objs.iter().enumerate() {
            let mut d = optdict.clone();
            let mut valid = true;
            for arg in ctx.param_list(&mut d)? {
                if !ctx.lookup_param(inst, &arg).map_err(ParseError::Raised)? {
                    valid = false;
                    break;
                }
            }
            if valid {
                ret.push(i);
            }
            ctx.check_leftover(&d)?;
        }
        Ok(ret)
    })();
    *lookup_prop(guest, def) = objs;
    r.map_err(|e| to_fail(def, optstr, e))
}

/// A stand-in guest for lookups, which never touch it.
static EMPTY: XmlClass = XmlClass {
    xml_name: "domain",
    props: &[],
    children: &[],
    prop_order: &[],
};

/// `_find_objects_to_edit`: an index (`1`, `-1`), `all`, or an option
/// string matched against each object.
pub fn find_objects_to_edit(
    def: &ParserDef,
    guest: &mut XmlObject,
    action_name: &str,
    editval: Option<&str>,
) -> Result<Vec<usize>, Error> {
    let n = lookup_prop(guest, def).len();
    let idx: Option<i64> = match editval {
        None => Some(1),
        Some(v) => {
            let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
            if digits(v) || v.strip_prefix('-').is_some_and(digits) {
                v.parse().ok()
            } else {
                None
            }
        }
    };
    if let Some(idx) = idx {
        // Edit device by index
        if idx == 0 {
            return Err(format!(
                "Invalid --edit option '{}'",
                editval.unwrap_or_default()
            ));
        }
        if n == 0 {
            return Err(format!(
                "No {} objects found in the XML",
                def.cli_flag_name()
            ));
        }
        if (n as i64) < idx.abs() {
            // ngettext's singular and plural forms.
            let (there, noun) = if n == 1 {
                ("there's", "object")
            } else {
                ("there are", "objects")
            };
            return Err(format!(
                "'--edit {idx}' requested but {there} only {n} {} {noun} in the XML",
                def.cli_flag_name()
            ));
        }
        let i = if idx > 0 { idx - 1 } else { n as i64 + idx };
        return Ok(vec![i as usize]);
    }
    let editval = editval.unwrap_or_default();
    if editval == "all" {
        return Ok((0..n).collect());
    }
    // Lookup device by the passed prop string
    let found = lookup_children(def, guest, editval)?;
    if found.is_empty() {
        return Err(format!(
            "No matching objects found for --{action_name} {editval}"
        ));
    }
    Ok(found)
}

/// `_add_common_device_args`.
pub fn add_common_device_args(def: &mut ParserDef, boot_loadparm: bool, virtio_options: bool) {
    for (cli, prop) in [
        ("address.type", "address.type"),
        ("address.domain", "address.domain"),
        ("address.bus", "address.bus"),
        ("address.slot", "address.slot"),
    ] {
        def.arg(ArgDef::new(cli, prop));
    }
    def.arg(ArgDef::new("address.multifunction", "address.multifunction").onoff());
    for (cli, prop) in [
        ("address.function", "address.function"),
        ("address.controller", "address.controller"),
        ("address.unit", "address.unit"),
        ("address.port", "address.port"),
        ("address.target", "address.target"),
        ("address.reg", "address.reg"),
        ("address.cssid", "address.cssid"),
        ("address.ssid", "address.ssid"),
        ("address.devno", "address.devno"),
        ("address.iobase", "address.iobase"),
        ("address.irq", "address.irq"),
        ("address.base", "address.base"),
        ("address.zpci.uid", "address.zpci_uid"),
        ("address.zpci.fid", "address.zpci_fid"),
        ("alias.name", "alias.name"),
    ] {
        def.arg(ArgDef::new(cli, prop));
    }
    if boot_loadparm {
        def.arg(ArgDef::new("boot.loadparm", "boot.loadparm"));
    }
    if virtio_options {
        def.arg(ArgDef::new("driver.ats", "virtio_driver.ats").onoff());
        def.arg(ArgDef::new("driver.iommu", "virtio_driver.iommu").onoff());
        def.arg(ArgDef::new("driver.packed", "virtio_driver.packed").onoff());
        def.arg(ArgDef::new("driver.page_per_vq", "virtio_driver.page_per_vq").onoff());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shlex_keeps_quoted_and_escaped_commas() {
        assert_eq!(shlex_split_commas("a=1,,b=2"), ["a=1", "b=2"]);
        assert_eq!(shlex_split_commas("path=\"/a,b\",x"), ["path=/a,b", "x"]);
        assert_eq!(shlex_split_commas("path='/a,b'"), ["path=/a,b"]);
        assert_eq!(shlex_split_commas("path=/a\\,b"), ["path=/a,b"]);
        assert_eq!(shlex_split_commas("x=''"), ["x="]);
        assert!(shlex_split_commas("").is_empty());
    }

    #[test]
    fn cliname_patterns_match_like_upstream_regexes() {
        assert!(pattern_matches("xpath[0-9]*.set", "xpath.set"));
        assert!(pattern_matches("xpath[0-9]*.set", "xpath12.set"));
        assert!(!pattern_matches("xpath[0-9]*.set", "xpathx.set"));
        assert!(pattern_matches("model", "model"));
        assert!(!pattern_matches("model", "model2"));
    }

    #[test]
    fn optdict_splices_remove_first_and_joins_can_comma() {
        let mut def = ParserDef::new("watchdog", Some("devices.watchdog"), None);
        def.remove_first = &["model"];
        def.arg(ArgDef::new("model", "model"));
        def.arg(ArgDef::new("action", "action"));
        let d = def.optdict("ib700,action=pause,model=x");
        assert_eq!(
            d.0,
            [
                ("model".to_string(), Some("x".to_string())),
                ("action".to_string(), Some("pause".to_string()))
            ]
        );
        let d = def.optdict("xpath.set=./a=1,b=2,action=x");
        assert_eq!(d.get("xpath.set"), Some(&Some("./a=1,b=2".to_string())));
    }
}
