// Copyright 2017 the V8 project authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef V8_BASE_TIMEZONE_CACHE_H_
#define V8_BASE_TIMEZONE_CACHE_H_

namespace v8 {
namespace base {

class TimezoneCache {
 public:
  // Short name of the local timezone (e.g., EST)
  virtual const char* LocalTimezone(double time_ms) = 0;

  // https://tc39.es/ecma262/#sec-daylight-saving-time-adjustment
  // Daylight Saving Time Adjustment
  virtual double DaylightSavingsOffset(double time_ms) = 0;

  // https://tc39.es/ecma262/#sec-local-time-zone-adjustment
  // Local Time Zone Adjustment
  //
  // https://github.com/tc39/ecma262/pull/778
  virtual double LocalTimeOffset(double time_ms, bool is_utc) = 0;

  /**
   * Time zone redetection indicator for Clear function.
   *
   * kSkip indicates host time zone doesn't have to be redetected.
   * kRedetect indicates host time zone should be redetected, and used to set
   * the default time zone.
   *
   * The host time zone detection may require file system access or similar
   * operations unlikely to be available inside a sandbox. If v8 is run inside a
   * sandbox, the host time zone has to be detected outside the sandbox
   * separately.
   */
  enum class TimeZoneDetection { kSkip, kRedetect };

  // Called when the local timezone changes
  virtual void Clear(TimeZoneDetection time_zone_detection) = 0;

  // Pin this cache to an IANA zone instead of the host's, or pass an empty
  // string to go back to the host's. Per cache, and a cache belongs to an
  // isolate -- which is the whole point: the ICU default this otherwise reads
  // is process-global, so one isolate cannot have a zone without giving every
  // other isolate the same one.
  //
  // Default is a no-op so that caches which have no way to honour it (the
  // non-ICU POSIX ones, which read the process environment) keep their existing
  // behaviour rather than silently claiming a zone they do not have.
  virtual void SetTimeZone(const char* iana_id) {}

  // Called when tearing down the isolate
  virtual ~TimezoneCache() = default;
};

}  // namespace base
}  // namespace v8

#endif  // V8_BASE_TIMEZONE_CACHE_H_
