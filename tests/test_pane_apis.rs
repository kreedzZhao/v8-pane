//! The two APIs this fork adds, each with the property that made it worth a patch.
//!
//! These run against a **source build** of this tree, which is the only build that has
//! them: `V8_FROM_SOURCE=1 cargo test --release --test test_pane_apis`. The release
//! profile is deliberate -- a debug source build produces a second, much larger V8 -- and
//! `pane-deps.sh` has to have run, because the ICU data file below is one of the two
//! things crates.io excludes.
use std::collections::BTreeMap;
use std::sync::Once;

fn initialize_once() {
  static START: Once = Once::new();
  START.call_once(|| {
    // Intl needs the data, and the zone assertions below are entirely about what Intl
    // answers. Without it `resolvedOptions().timeZone` degrades to UTC and both halves of
    // the test would agree for the wrong reason.
    assert!(
      v8::icu::set_common_data_78(align_data::include_aligned!(
        align_data::Align16,
        "../third_party/icu/common/icudtl.dat"
      ))
      .is_ok()
    );
    v8::V8::initialize_platform(
      v8::new_unprotected_default_platform(0, false).make_shared(),
    );
    v8::V8::initialize();
  });
}

/// The zone as the two surfaces that must agree report it: the IANA name `Intl` resolves,
/// and the minutes behind UTC `Date` reports at a fixed instant, so daylight saving is not
/// a variable.
///
/// January 15th, because `Pacific/Chatham` is +13:45 there -- `-825`, a number no
/// whole-hour zone can produce in either direction.
fn surfaces(isolate: &mut v8::OwnedIsolate) -> (String, String) {
  unsafe { isolate.enter() };
  let answer = read(isolate);
  unsafe { isolate.exit() };
  answer
}

fn read(isolate: &mut v8::OwnedIsolate) -> (String, String) {
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, v8::ContextOptions::default());
  let mut scope = v8::ContextScope::new(scope, context);
  let scope = &mut scope;
  (
    eval_string(scope, "Intl.DateTimeFormat().resolvedOptions().timeZone"),
    eval_string(
      scope,
      "String(new Date(Date.UTC(2026, 0, 15, 12)).getTimezoneOffset())",
    ),
  )
}

fn eval_string(scope: &mut v8::PinScope, source: &str) -> String {
  let code = v8::String::new(scope, source).expect("the source is a string");
  let script = v8::Script::compile(scope, code, None).expect("it compiles");
  let value = script.run(scope).expect("it runs");
  value.to_rust_string_lossy(scope)
}

fn isolate_in(zone: &str) -> v8::OwnedIsolate {
  let mut isolate = v8::Isolate::new(v8::CreateParams::default());
  assert!(isolate.set_time_zone(Some(zone)), "{zone} was refused");
  isolate
}

/// One expression, evaluated in a fresh context of `isolate`, entered and exited around
/// it so that several isolates can take turns on one thread.
fn eval_in(isolate: &mut v8::OwnedIsolate, source: &str) -> String {
  unsafe { isolate.enter() };
  let answer = {
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, v8::ContextOptions::default());
    let mut scope = v8::ContextScope::new(scope, context);
    eval_string(&mut scope, source)
  };
  unsafe { isolate.exit() };
  answer
}

/// The zone as all three surfaces a page reads it through report it: the name `Intl`
/// resolves with no `timeZone` option, the minutes behind UTC `Date` reports on January
/// 15th, and `Temporal.Now.timeZoneId()`. Each reaches the zone by its own road -- ICU
/// formatter, the isolate's `DateCache`, `Intl::DefaultTimeZone` -- so agreeing is not
/// automatic.
fn three(isolate: &mut v8::OwnedIsolate) -> [String; 3] {
  let joined = eval_in(
    isolate,
    "[Intl.DateTimeFormat().resolvedOptions().timeZone, \
      String(new Date(Date.UTC(2026, 0, 15, 12)).getTimezoneOffset()), \
      Temporal.Now.timeZoneId()].join('|')",
  );
  let parts: Vec<String> = joined.split('|').map(str::to_owned).collect();
  parts.try_into().expect("three fields")
}

/// What a page would get from `new Intl.DateTimeFormat(undefined, {timeZone: name})`:
/// `None` for a `RangeError`, otherwise the name `resolvedOptions()` reports and the
/// minutes behind UTC on January 15th (read from `longOffset`, so through the formatter
/// rather than through `Date`). This is the oracle `set_time_zone` is held to.
fn intl_would_say(
  isolate: &mut v8::OwnedIsolate,
  name: &str,
) -> Option<(String, String)> {
  // Debug-quoting is a valid JS string literal for the ASCII names used here.
  let source = format!(
    r#"(() => {{
      const name = {name:?};
      let f;
      try {{ f = new Intl.DateTimeFormat(undefined, {{timeZone: name}}); }}
      catch (e) {{ return e instanceof RangeError ? "RangeError" : "other: " + e; }}
      const part = new Intl.DateTimeFormat("en-US", {{timeZone: name, timeZoneName: "longOffset"}})
        .formatToParts(new Date(Date.UTC(2026, 0, 15, 12)))
        .find(p => p.type === "timeZoneName").value;
      const m = /^GMT(?:([+−-])(\d\d):(\d\d))?$/.exec(part);
      const east = m[1] ? (m[1] === "+" ? 1 : -1) * (Number(m[2]) * 60 + Number(m[3])) : 0;
      return f.resolvedOptions().timeZone + "|" + String(-east);
    }})()"#
  );
  let answer = eval_in(isolate, &source);
  if answer == "RangeError" {
    return None;
  }
  let (zone, offset) = answer
    .split_once('|')
    .unwrap_or_else(|| panic!("{name}: the oracle answered {answer}"));
  Some((zone.to_owned(), offset.to_owned()))
}

/// `set_time_zone` accepts what `Intl.DateTimeFormat`'s `timeZone` option accepts, refuses
/// what it refuses, and stores the name `resolvedOptions().timeZone` reports -- because the
/// same V8 code decides both. The middle column is the observed V8 behaviour, pinned so a
/// roll that changes it is noticed; the oracle column is asked of V8 at run time, so a
/// mismatch between the two says which one moved.
#[test]
fn set_time_zone_accepts_and_names_zones_the_way_intl_does() {
  initialize_once();

  let cases: &[(&str, Option<&str>)] = &[
    ("Asia/Tokyo", Some("Asia/Tokyo")),
    // ASCII case-insensitive, stored in canonical case.
    ("asia/tokyo", Some("Asia/Tokyo")),
    ("AMERICA/NEW_YORK", Some("America/New_York")),
    // Aliases resolve the way ICU's canonical-ID table resolves them.
    ("Asia/Calcutta", Some("Asia/Calcutta")),
    ("Asia/Kolkata", Some("Asia/Calcutta")),
    ("EST5EDT", Some("America/New_York")),
    // The UTC family.
    ("UTC", Some("UTC")),
    ("utc", Some("UTC")),
    ("GMT", Some("UTC")),
    ("Etc/GMT", Some("UTC")),
    ("Etc/GMT+5", Some("Etc/GMT+5")),
    // Offset strings, as ECMA-402 allows them.
    ("+05:30", Some("+05:30")),
    ("-0800", Some("-08:00")),
    // Refused.
    ("Foo/Bar", None),
    ("UTC+5", None),
    ("GMT+5", None),
    ("+24:00", None),
    (" Asia/Tokyo", None),
  ];

  let mut table = Vec::new();
  for &(name, expected) in cases {
    // Start from a pinned zone, so "refused" can be told apart from "pinned to UTC".
    let mut isolate = isolate_in("Pacific/Chatham");
    let before = three(&mut isolate);
    let oracle = intl_would_say(&mut isolate, name);
    let took = isolate.set_time_zone(Some(name));
    let stored = isolate.time_zone();
    let after = three(&mut isolate);
    table.push(format!(
      "{name:>18} -> {took:5} stored={stored:?} surfaces={after:?} intl={oracle:?}"
    ));

    assert_eq!(
      took,
      oracle.is_some(),
      "{name}: set_time_zone and Intl.DateTimeFormat disagree about accepting it"
    );
    assert_eq!(
      stored.as_deref(),
      Some(expected.unwrap_or("Pacific/Chatham")),
      "{name}: stored name"
    );
    match oracle {
      Some((zone, offset)) => {
        assert_eq!(Some(zone.as_str()), expected, "{name}: Intl's own answer moved");
        assert_eq!(
          after,
          [zone.clone(), offset, zone],
          "{name}: the three surfaces do not all say the zone Intl resolves the name to"
        );
      }
      None => {
        assert_eq!(expected, None, "{name}: Intl now refuses it");
        assert_eq!(after, before, "{name}: a refused name moved the isolate");
      }
    }
  }
  eprintln!("{}", table.join("\n"));
}

/// A refused name leaves the isolate exactly where it was -- pinned or not -- on all three
/// surfaces, and `None` takes a pinned isolate back to the host's zone.
#[test]
fn a_refused_zone_changes_nothing_and_none_goes_back_to_the_host() {
  initialize_once();

  let mut host = v8::Isolate::new(v8::CreateParams::default());
  let host_zone = three(&mut host);
  assert_eq!(host.time_zone(), None);

  let refused = ["Foo/Bar", "UTC+5", "Asia/Tokyo\0Europe/London", "\0"];

  // Pinned: a refusal keeps the pin.
  let mut pinned = isolate_in("Pacific/Chatham");
  let chatham = three(&mut pinned);
  assert_eq!(
    chatham,
    ["Pacific/Chatham".to_owned(), "-825".to_owned(), "Pacific/Chatham".to_owned()]
  );
  for name in refused {
    assert!(!pinned.set_time_zone(Some(name)), "{name:?} was accepted");
    assert_eq!(pinned.time_zone().as_deref(), Some("Pacific/Chatham"), "{name:?}");
    assert_eq!(three(&mut pinned), chatham, "{name:?} moved a pinned isolate");
  }

  // Unpinned: a refusal does not pin it to anything, UTC included.
  for name in refused {
    assert!(!host.set_time_zone(Some(name)), "{name:?} was accepted");
    assert_eq!(host.time_zone(), None, "{name:?}");
    assert_eq!(three(&mut host), host_zone, "{name:?} moved an unpinned isolate");
  }

  // `None` and the empty string both mean the host's zone, and both succeed.
  assert!(pinned.set_time_zone(None));
  assert_eq!(pinned.time_zone(), None);
  assert_eq!(three(&mut pinned), host_zone);
  assert!(pinned.set_time_zone(Some("Asia/Tokyo")));
  assert_eq!(
    three(&mut pinned),
    ["Asia/Tokyo".to_owned(), "-540".to_owned(), "Asia/Tokyo".to_owned()]
  );
  assert!(pinned.set_time_zone(Some("")));
  assert_eq!(pinned.time_zone(), None);
  assert_eq!(three(&mut pinned), host_zone);
}

/// Two isolates on two threads, each pinned to its own zone and reading it in lockstep with
/// the other, so every read of one falls between reads of the other. Neither ever reports
/// the other's zone on any surface, and an unpinned isolate on a third thread keeps the host's.
#[test]
fn isolates_on_two_threads_keep_their_own_zones_while_interleaved() {
  initialize_once();

  let mut host = v8::Isolate::new(v8::CreateParams::default());
  let host_zone = three(&mut host);

  const ROUNDS: usize = 25;
  let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
  let spawn = |zone: &'static str, offset: &'static str| {
    let barrier = barrier.clone();
    std::thread::spawn(move || {
      let mut isolate = isolate_in(zone);
      let want = [zone.to_owned(), offset.to_owned(), zone.to_owned()];
      for round in 0..ROUNDS {
        barrier.wait();
        assert_eq!(three(&mut isolate), want, "{zone}, round {round}");
      }
    })
  };
  let a = spawn("Pacific/Chatham", "-825");
  let b = spawn("America/New_York", "300");
  a.join().expect("the Chatham thread");
  b.join().expect("the New York thread");

  assert_eq!(three(&mut host), host_zone, "an unpinned isolate moved");
}

/// Why this API exists at all: upstream's only lever is the *process's* zone plus a
/// redetect notification, and with two isolates alive that is not merely inconvenient but
/// wrong. `Date` latches per isolate at the notification while `Intl` reads ICU's global
/// at every construction, and any isolate's notification resets that global -- so one
/// isolate reports another's zone through one surface and its own through the other.
#[test]
fn two_live_isolates_each_answer_their_own_zone() {
  initialize_once();

  // An isolate with no zone of its own, read *before* anything sets one, so that whatever
  // the host zone is on the machine running this, it is on the record.
  let mut host = v8::Isolate::new(v8::CreateParams::default());
  let host_before = surfaces(&mut host);

  // Both built before either is read. Reading A first would hide the defect this exists
  // for: A's own notification was the last one, so the global default was still A's zone
  // until B existed.
  let mut a = isolate_in("Pacific/Chatham");
  let mut b = isolate_in("America/New_York");

  let mut said = BTreeMap::new();
  said.insert("A", surfaces(&mut a));
  said.insert("B", surfaces(&mut b));

  let expected: BTreeMap<&str, (&str, &str)> = [
    ("A", ("Pacific/Chatham", "-825")),
    ("B", ("America/New_York", "300")),
  ]
  .into_iter()
  .collect();

  for (name, (zone, offset)) in &expected {
    let (got_zone, got_offset) = said.get(name).expect("both were read");
    // Asserted apart so a failure says which half moved. Before the patch it was the
    // first: Intl followed whichever isolate notified last, and Date did not.
    assert_eq!(
      got_zone, zone,
      "isolate {name}'s Intl reports {got_zone}, not the zone it was given -- it is \
       reading ICU's process-global default, which the other isolate moved"
    );
    assert_eq!(
      got_offset, offset,
      "isolate {name}'s Date reports {got_offset} minutes behind UTC, not the zone it \
       was given"
    );
  }

  // The half a process-global mechanism can never deliver: an isolate that asked for no
  // zone still has the host's, *after* two others took their own. If SetTimeZone wrote
  // anything outside its isolate -- ICU's default, TZ -- this is where it would show.
  let host_after = surfaces(&mut host);
  assert_eq!(
    host_after, host_before,
    "an isolate with no zone of its own moved when two others took theirs, so the zone \
     is still process state somewhere"
  );
}

/// `MarkAsUndetectable` is public V8 API that upstream's bindings do not expose. It is the
/// only way to build what `document.all` is: an object that answers `typeof` with
/// `"undefined"` and is falsy while not being `undefined`. Nothing in JavaScript
/// reproduces it -- `typeof` on an ordinary object, or on a `Proxy`, never says
/// `"undefined"` -- so it is a one-line test for a real browser.
#[test]
fn an_undetectable_instance_answers_typeof_undefined_without_being_undefined() {
  initialize_once();

  let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, v8::ContextOptions::default());
  let mut scope = v8::ContextScope::new(scope, context);
  let scope = &mut scope;

  let template = v8::ObjectTemplate::new(scope);
  // V8 refuses the flag on a template with no call handler --
  // `Check failed: !IsUndefined(obj->GetInstanceCallHandler())` -- and it is right to:
  // the object this models is callable. `document.all(x)` is a legacy caller.
  template.set_call_as_function_handler(
    |_: &mut v8::PinScope,
     _: v8::FunctionCallbackArguments,
     _: v8::ReturnValue<v8::Value>| {},
    None,
  );
  template.mark_as_undetectable();

  let instance = template.new_instance(scope).expect("an instance");
  let key = v8::String::new(scope, "all").expect("a key");
  let global = context.global(scope);
  global.set(scope, key.into(), instance.into());

  assert_eq!(eval_string(scope, "typeof all"), "undefined");
  assert_eq!(eval_string(scope, "String(all === undefined)"), "false");
  assert_eq!(eval_string(scope, "String(!all)"), "true");

  // An ordinary instance of the same template shape is the control: it is the flag that
  // does this, not the call handler and not the template.
  let plain = v8::ObjectTemplate::new(scope);
  let plain = plain.new_instance(scope).expect("an instance");
  let key = v8::String::new(scope, "plain").expect("a key");
  global.set(scope, key.into(), plain.into());
  assert_eq!(eval_string(scope, "typeof plain"), "object");
}

/// `Template::SetLazyDataProperty`, bound here because upstream binds it on `v8::Object`
/// only, and the two differ in the one thing an embedder cannot fix afterwards:
/// **position**. An `Object`-level install runs after `Context::New` and appends to the
/// object's own-property order; a template-level install is part of instantiation.
///
/// What instantiation does with it is a property of this tree, so it is pinned here:
/// `ConfigureInstance` (`v8/src/api/api-natives.cc`) installs a template's accessor group
/// -- which lazy properties join -- before its `Template::Set` group, and installs that
/// first group in reverse. An embedder that needs names in a *captured* order therefore
/// cannot mix the two calls on one template and keep it; what it can rely on, and the
/// second half asserts, is that the property is not observably lazy: enumeration does not
/// run the getter, and the unread descriptor is an ordinary data descriptor.
#[test]
fn a_template_lazy_property_installs_first_stays_lazy_and_describes_as_data() {
  initialize_once();

  static RAN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
  fn getter(
    scope: &mut v8::PinScope<'_, '_>,
    _key: v8::Local<'_, v8::Name>,
    _args: v8::PropertyCallbackArguments<'_>,
    mut rv: v8::ReturnValue<v8::Value>,
  ) {
    RAN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let answer = v8::Integer::new(scope, 7);
    rv.set(answer.into());
  }

  let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
  v8::scope!(let scope, isolate);

  // One plain name on either side of the lazy one, so both facts are visible at once:
  // the lazy name leaves its insertion slot, and the plain group keeps its order.
  let template = v8::ObjectTemplate::new(scope);
  for name in ["alpha", "omega"] {
    let key = v8::String::new(scope, name).expect("a key");
    let value = v8::Integer::new(scope, 1);
    if name == "alpha" {
      template.set_with_attr(key.into(), value.into(), v8::PropertyAttribute::NONE);
      let lazy_key = v8::String::new(scope, "middle").expect("a key");
      template.set_lazy_data_property(
        lazy_key.into(),
        getter,
        None,
        v8::PropertyAttribute::NONE,
        v8::SideEffectType::HasNoSideEffect,
        v8::SideEffectType::HasSideEffect,
      );
    } else {
      template.set_with_attr(key.into(), value.into(), v8::PropertyAttribute::NONE);
    }
  }

  let context = v8::Context::new(scope, v8::ContextOptions::default());
  let mut scope = v8::ContextScope::new(scope, context);
  let scope = &mut scope;
  let instance = template.new_instance(scope).expect("an instance");
  let key = v8::String::new(scope, "it").expect("a key");
  context.global(scope).set(scope, key.into(), instance.into());

  // The grouping rule. If this moves, the embedder's ordering workaround (installing on
  // the live object instead) may no longer be necessary -- re-derive before relying on it.
  assert_eq!(
    eval_string(scope, "JSON.stringify(Object.getOwnPropertyNames(it))"),
    r#"["middle","alpha","omega"]"#,
  );
  assert_eq!(RAN.load(std::sync::atomic::Ordering::SeqCst), 0, "enumeration ran the getter");

  // Unread, it describes as plain data -- `{value}`, not `{get,set}` -- which is what
  // makes laziness unobservable to one line of script.
  assert_eq!(
    eval_string(
      scope,
      "JSON.stringify(Object.getOwnPropertyDescriptor(it, 'middle'))",
    ),
    r#"{"value":7,"writable":true,"enumerable":true,"configurable":true}"#,
  );
  assert_eq!(RAN.load(std::sync::atomic::Ordering::SeqCst), 1);

  // Reading it replaces it with that value; the getter does not run again.
  assert_eq!(eval_string(scope, "String(it.middle)"), "7");
  assert_eq!(eval_string(scope, "String(it.middle === it.middle)"), "true");
  assert_eq!(RAN.load(std::sync::atomic::Ordering::SeqCst), 1, "a second read ran the getter again");
}
