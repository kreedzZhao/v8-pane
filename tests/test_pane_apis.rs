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
  isolate.set_time_zone(Some(zone));
  isolate
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
