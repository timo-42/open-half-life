#include "ohl/media/payload_stream.hpp"

#include <cstddef>
#include <cstdint>
#include <initializer_list>
#include <iostream>
#include <limits>
#include <span>
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
  SyntheticSource(std::initializer_list<std::vector<std::byte>> chunks,
                  const bool succeed = true)
      : chunks_(chunks), succeed_(succeed) {}

  [[nodiscard]] bool stream(
      const std::uint64_t source_token,
      ohl::media::PayloadByteSink& sink) noexcept override {
    observed_token_ = source_token;
    for (const auto& chunk : chunks_) {
      if (!sink.write(chunk)) {
        return false;
      }
    }
    return succeed_;
  }

  [[nodiscard]] std::uint64_t observed_token() const noexcept {
    return observed_token_;
  }

 private:
  std::vector<std::vector<std::byte>> chunks_;
  bool succeed_{true};
  std::uint64_t observed_token_{0};
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
    std::declval<std::uint64_t>(),
    std::declval<ohl::media::PayloadByteSink&>())));
static_assert(noexcept(ohl::media::stream_payload_entry(
    std::declval<const ohl::media::PlannedPayloadEntry&>(),
    std::declval<ohl::media::PayloadSource&>(),
    std::declval<ohl::media::PayloadByteSink&>())));

int main() {
  using ohl::media::PayloadStreamError;
  using ohl::media::PlannedPayloadEntry;

  {
    const PlannedPayloadEntry entry{42, "unused/destination", 3};
    SyntheticSource source{{bytes({1, 2, 3})}};
    RecordingSink destination;
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!result.complete() || result.bytes_written != 3 ||
        source.observed_token() != 42 || destination.bytes() != bytes({1, 2, 3})) {
      std::cerr << "exact payload stream failed\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{7, "not/source-visible", 4};
    SyntheticSource source{{bytes({1}), bytes({2, 3}), bytes({4})}};
    RecordingSink destination;
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!result.complete() || destination.bytes() != bytes({1, 2, 3, 4}) ||
        destination.calls() != 3) {
      std::cerr << "chunked payload stream failed\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{8, "empty", 0};
    SyntheticSource source(std::initializer_list<std::vector<std::byte>>{});
    RecordingSink destination;
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!result.complete() || result.bytes_written != 0 ||
        destination.calls() != 0) {
      std::cerr << "zero-byte payload stream failed\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{9, "empty-chunks", 2};
    SyntheticSource source{{bytes({}), bytes({1, 2}), bytes({})}};
    RecordingSink destination;
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!result.complete() || result.bytes_written != 2 ||
        destination.bytes() != bytes({1, 2}) || destination.calls() != 3) {
      std::cerr << "empty chunks around exact completion failed\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{10, "short", 3};
    SyntheticSource source{{bytes({1, 2})}};
    RecordingSink destination;
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!is_result(result, PayloadStreamError::underflow, 2)) {
      std::cerr << "payload underflow was not reported\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{11, "long", 3};
    SyntheticSource source{{bytes({1, 2}), bytes({3, 4})}};
    RecordingSink destination;
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!is_result(result, PayloadStreamError::overflow, 2) ||
        destination.bytes() != bytes({1, 2}) || destination.calls() != 1) {
      std::cerr << "payload overflow reached the destination\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{12, "first-chunk-overflow", 2};
    SyntheticSource source{{bytes({1, 2, 3})}};
    RecordingSink destination;
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!is_result(result, PayloadStreamError::overflow, 0) ||
        destination.calls() != 0 || !destination.bytes().empty()) {
      std::cerr << "first-chunk overflow reached the destination\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{13, "destination-failure", 3};
    SyntheticSource source{{bytes({1}), bytes({2, 3})}};
    RecordingSink destination{2};
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!is_result(result, PayloadStreamError::destination_failure, 1) ||
        destination.bytes() != bytes({1})) {
      std::cerr << "destination failure was not reported\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{14, "source-failure", 3};
    SyntheticSource source{{bytes({1})}, false};
    RecordingSink destination;
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!is_result(result, PayloadStreamError::source_failure, 1)) {
      std::cerr << "source failure was not reported\n";
      return 1;
    }
  }

  {
    const PlannedPayloadEntry entry{15, "source-failure-after-exact", 2};
    SyntheticSource source{{bytes({1, 2})}, false};
    RecordingSink destination;
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!is_result(result, PayloadStreamError::source_failure, 2) ||
        destination.bytes() != bytes({1, 2})) {
      std::cerr << "source failure after exact bytes was not reported\n";
      return 1;
    }
  }

  {
    constexpr auto maximum_token = std::numeric_limits<std::uint64_t>::max();
    const PlannedPayloadEntry entry{maximum_token, "maximum-token", 0};
    SyntheticSource source(std::initializer_list<std::vector<std::byte>>{});
    RecordingSink destination;
    const auto result =
        ohl::media::stream_payload_entry(entry, source, destination);
    if (!result.complete() || source.observed_token() != maximum_token) {
      std::cerr << "maximum source token was not forwarded intact\n";
      return 1;
    }
  }

  return 0;
}
