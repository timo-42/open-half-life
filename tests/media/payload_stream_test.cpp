#include "ohl/media/payload_stream.hpp"

#include "synthetic_media_test_support.hpp"

#include <cstddef>
#include <cstdint>
#include <initializer_list>
#include <iostream>
#include <limits>
#include <span>
#include <stop_token>
#include <utility>
#include <vector>

namespace {

class RecordingSink final : public ohl::media::PayloadByteSink {
 public:
  explicit RecordingSink(const std::size_t fail_on_call = 0)
      : fail_on_call_(fail_on_call) {}

  [[nodiscard]] bool write(
      const std::span<const std::byte> bytes) noexcept override {
    ++calls_;
    if (fail_on_call_ != 0 && calls_ == fail_on_call_) {
      return false;
    }
    bytes_.insert(bytes_.end(), bytes.begin(), bytes.end());
    return true;
  }

  [[nodiscard]] const std::vector<std::byte>& bytes() const noexcept {
    return bytes_;
  }
  [[nodiscard]] std::size_t calls() const noexcept { return calls_; }

 private:
  std::size_t fail_on_call_{0};
  std::size_t calls_{0};
  std::vector<std::byte> bytes_;
};

class SyntheticSource final : public ohl::media::PayloadSource {
 public:
  SyntheticSource(
      const ohl::platform::MediaSource& expected_media_source,
      std::initializer_list<std::vector<std::byte>> chunks,
      const bool succeed = true, const std::stop_token expected_stop_token = {},
      std::stop_source* request_stop = nullptr,
      const std::size_t request_after_chunk = 0,
      const bool request_before_return = false)
      : expected_media_source_(&expected_media_source),
        chunks_(chunks),
        succeed_(succeed),
        expected_stop_token_(expected_stop_token),
        request_stop_(request_stop),
        request_after_chunk_(request_after_chunk),
        request_before_return_(request_before_return) {}

  [[nodiscard]] bool stream(
      const ohl::platform::MediaSource& media_source,
      const std::uint64_t source_token, const std::stop_token stop_token,
      ohl::media::PayloadByteSink& sink) noexcept override {
    ++calls_;
    observed_media_source_ = &media_source;
    observed_token_ = source_token;
    observed_stop_token_ = stop_token;
    observed_sink_ = &sink;
    contract_ok_ = &media_source == expected_media_source_ &&
                   stop_token == expected_stop_token_ && observed_sink_ != nullptr;
    if (!contract_ok_) {
      return false;
    }
    for (std::size_t index = 0; index < chunks_.size(); ++index) {
      if (!sink.write(chunks_[index])) {
        return false;
      }
      if (request_stop_ != nullptr && request_after_chunk_ == index + 1) {
        (void)request_stop_->request_stop();
      }
    }
    if (request_stop_ != nullptr && request_before_return_) {
      (void)request_stop_->request_stop();
    }
    return succeed_;
  }

  [[nodiscard]] bool contract_ok() const noexcept { return contract_ok_; }
  [[nodiscard]] std::size_t calls() const noexcept { return calls_; }
  [[nodiscard]] const ohl::platform::MediaSource* observed_media_source()
      const noexcept {
    return observed_media_source_;
  }
  [[nodiscard]] std::uint64_t observed_token() const noexcept {
    return observed_token_;
  }
  [[nodiscard]] std::stop_token observed_stop_token() const noexcept {
    return observed_stop_token_;
  }
  [[nodiscard]] const ohl::media::PayloadByteSink* observed_sink()
      const noexcept {
    return observed_sink_;
  }

 private:
  const ohl::platform::MediaSource* expected_media_source_{nullptr};
  std::vector<std::vector<std::byte>> chunks_;
  bool succeed_{true};
  std::stop_token expected_stop_token_;
  std::stop_source* request_stop_{nullptr};
  std::size_t request_after_chunk_{0};
  bool request_before_return_{false};
  bool contract_ok_{false};
  std::size_t calls_{0};
  const ohl::platform::MediaSource* observed_media_source_{nullptr};
  std::uint64_t observed_token_{0};
  std::stop_token observed_stop_token_;
  const ohl::media::PayloadByteSink* observed_sink_{nullptr};
};

[[nodiscard]] std::vector<std::byte> bytes(
    std::initializer_list<unsigned int> values) {
  std::vector<std::byte> result;
  result.reserve(values.size());
  for (const auto value : values) {
    result.push_back(static_cast<std::byte>(value));
  }
  return result;
}

[[nodiscard]] bool is_result(const ohl::media::PayloadStreamResult& result,
                             const ohl::media::PayloadStreamError error,
                             const std::uint64_t bytes_written) {
  return result.error == error && result.bytes_written == bytes_written;
}

}  // namespace

static_assert(noexcept(std::declval<ohl::media::PayloadByteSink&>().write(
    std::declval<std::span<const std::byte>>())));
static_assert(noexcept(std::declval<ohl::media::PayloadSource&>().stream(
    std::declval<const ohl::platform::MediaSource&>(),
    std::declval<std::uint64_t>(), std::declval<std::stop_token>(),
    std::declval<ohl::media::PayloadByteSink&>())));
static_assert(noexcept(ohl::media::stream_payload_entry(
    std::declval<const ohl::media::PlannedPayloadEntry&>(),
    std::declval<const ohl::platform::MediaSource&>(),
    std::declval<ohl::media::PayloadSource&>(),
    std::declval<std::stop_token>(),
    std::declval<ohl::media::PayloadByteSink&>())));

int main() {
  using ohl::media::PayloadStreamError;
  using ohl::media::PlannedPayloadEntry;

  ohl::media::test::SyntheticValidatedMedia fixture;
  const auto& media_source = *fixture.media().source();

  {
    const PlannedPayloadEntry entry{42, "unused/destination", 3};
    SyntheticSource source{media_source, {bytes({1, 2, 3})}};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!result.complete() || result.bytes_written != 3 ||
        !source.contract_ok() || source.observed_media_source() != &media_source ||
        source.observed_token() != 42 || source.observed_stop_token() !=
                                                   std::stop_token{} ||
        source.observed_sink() == nullptr ||
        source.observed_sink() == &destination ||
        destination.bytes() != bytes({1, 2, 3})) {
      std::cerr << "exact payload stream contract failed\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{7, "not/source-visible", 4};
    SyntheticSource source{media_source,
                           {bytes({1}), bytes({2, 3}), bytes({4})}};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!result.complete() || !source.contract_ok() ||
        destination.bytes() != bytes({1, 2, 3, 4}) ||
        destination.calls() != 3) {
      std::cerr << "chunked payload stream failed\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{8, "empty", 0};
    SyntheticSource source{
        media_source, std::initializer_list<std::vector<std::byte>>{}};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!result.complete() || result.bytes_written != 0 ||
        !source.contract_ok() || destination.calls() != 0) {
      std::cerr << "zero-byte payload stream failed\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{9, "empty-chunks", 2};
    SyntheticSource source{media_source,
                           {bytes({}), bytes({1, 2}), bytes({})}};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!result.complete() || result.bytes_written != 2 ||
        destination.bytes() != bytes({1, 2}) || destination.calls() != 3) {
      std::cerr << "empty chunks around exact completion failed\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{10, "short", 3};
    SyntheticSource source{media_source, {bytes({1, 2})}};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!is_result(result, PayloadStreamError::underflow, 2)) {
      std::cerr << "payload underflow was not reported\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{11, "long", 3};
    SyntheticSource source{media_source, {bytes({1, 2}), bytes({3, 4})}};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!is_result(result, PayloadStreamError::overflow, 2) ||
        destination.bytes() != bytes({1, 2}) || destination.calls() != 1) {
      std::cerr << "payload overflow reached the destination\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{12, "first-chunk-overflow", 2};
    SyntheticSource source{media_source, {bytes({1, 2, 3})}};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!is_result(result, PayloadStreamError::overflow, 0) ||
        destination.calls() != 0 || !destination.bytes().empty()) {
      std::cerr << "first-chunk overflow reached the destination\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{13, "destination-failure", 3};
    SyntheticSource source{media_source, {bytes({1}), bytes({2, 3})}};
    RecordingSink destination{2};
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!is_result(result, PayloadStreamError::destination_failure, 1) ||
        destination.bytes() != bytes({1})) {
      std::cerr << "destination failure was not reported\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{14, "source-failure", 3};
    SyntheticSource source{media_source, {bytes({1})}, false};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!is_result(result, PayloadStreamError::source_failure, 1)) {
      std::cerr << "source failure was not reported\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{15, "source-failure-after-exact", 2};
    SyntheticSource source{media_source, {bytes({1, 2})}, false};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!is_result(result, PayloadStreamError::source_failure, 2) ||
        destination.bytes() != bytes({1, 2})) {
      std::cerr << "source failure after exact bytes was not reported\n";
      return 1;
    }
  }

  {
    constexpr auto maximum_token = std::numeric_limits<std::uint64_t>::max();
    const PlannedPayloadEntry entry{maximum_token, "maximum-token", 0};
    SyntheticSource source{
        media_source, std::initializer_list<std::vector<std::byte>>{}};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, {}, destination);
    if (!result.complete() || source.observed_token() != maximum_token) {
      std::cerr << "maximum source token was not forwarded intact\n";
      return 1;
    }
  }

  {
    std::stop_source stop;
    (void)stop.request_stop();
    const PlannedPayloadEntry entry{21, "pre-cancel", 1};
    SyntheticSource source{media_source, {bytes({1})}, true, stop.get_token()};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, stop.get_token(), destination);
    if (!is_result(result, PayloadStreamError::cancelled, 0) ||
        source.calls() != 0 || destination.calls() != 0) {
      std::cerr << "pre-cancelled stream mutated source or destination\n";
      return 1;
    }
  }

  {
    std::stop_source stop;
    const PlannedPayloadEntry entry{22, "cancel-during", 3};
    SyntheticSource source{media_source,
                           {bytes({1}), bytes({2, 3, 4})}, true,
                           stop.get_token(), &stop, 1};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, stop.get_token(), destination);
    if (!is_result(result, PayloadStreamError::cancelled, 1) ||
        destination.bytes() != bytes({1}) || destination.calls() != 1) {
      std::cerr << "during-stream cancellation or bounded precedence failed\n";
      return 1;
    }
  }

  {
    std::stop_source stop;
    const PlannedPayloadEntry entry{23, "cancel-after", 2};
    SyntheticSource source{media_source, {bytes({1, 2})}, true,
                           stop.get_token(), &stop, 0, true};
    RecordingSink destination;
    const auto result = ohl::media::stream_payload_entry(
        entry, media_source, source, stop.get_token(), destination);
    if (!is_result(result, PayloadStreamError::cancelled, 2) ||
        destination.bytes() != bytes({1, 2})) {
      std::cerr << "post-stream cancellation was not observed\n";
      return 1;
    }
  }

  return 0;
}
