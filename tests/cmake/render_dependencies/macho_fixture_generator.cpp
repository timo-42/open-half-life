#include <cstdint>
#include <filesystem>
#include <fstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
constexpr std::uint32_t cpu_arm64 = 0x0100000cU;
constexpr std::uint32_t cpu_x86_64 = 0x01000007U;
constexpr std::uint32_t mh_dylib = 6U;

void append_u32(std::vector<unsigned char>& bytes, std::uint32_t value, bool little)
{
    for (int index = 0; index < 4; ++index) {
        const int shift = little ? index * 8 : (3 - index) * 8;
        bytes.push_back(static_cast<unsigned char>((value >> shift) & 0xffU));
    }
}

void append_u64(std::vector<unsigned char>& bytes, std::uint64_t value, bool little)
{
    for (int index = 0; index < 8; ++index) {
        const int shift = little ? index * 8 : (7 - index) * 8;
        bytes.push_back(static_cast<unsigned char>((value >> shift) & 0xffU));
    }
}

std::vector<unsigned char> thin(std::uint32_t cpu, std::uint32_t type)
{
    std::vector<unsigned char> bytes;
    append_u32(bytes, 0xfeedfacfU, true);
    append_u32(bytes, cpu, true);
    append_u32(bytes, 0U, true);
    append_u32(bytes, type, true);
    append_u32(bytes, 0U, true);
    append_u32(bytes, 0U, true);
    append_u32(bytes, 0U, true);
    append_u32(bytes, 0U, true);
    return bytes;
}

std::vector<unsigned char> fat32(
    bool little,
    const std::vector<std::pair<std::uint32_t, std::vector<unsigned char>>>& slices
)
{
    std::vector<unsigned char> bytes;
    append_u32(bytes, little ? 0xbebafecaU : 0xcafebabeU, false);
    append_u32(bytes, static_cast<std::uint32_t>(slices.size()), little);
    std::uint32_t offset = 8U + static_cast<std::uint32_t>(20U * slices.size());
    for (const auto& [cpu, slice] : slices) {
        append_u32(bytes, cpu, little);
        append_u32(bytes, 0U, little);
        append_u32(bytes, offset, little);
        append_u32(bytes, static_cast<std::uint32_t>(slice.size()), little);
        append_u32(bytes, 0U, little);
        offset += static_cast<std::uint32_t>(slice.size());
    }
    for (const auto& [cpu, slice] : slices) {
        static_cast<void>(cpu);
        bytes.insert(bytes.end(), slice.begin(), slice.end());
    }
    return bytes;
}

std::vector<unsigned char> fat64(
    bool little,
    const std::vector<std::pair<std::uint32_t, std::vector<unsigned char>>>& slices
)
{
    std::vector<unsigned char> bytes;
    append_u32(bytes, little ? 0xbfbafecaU : 0xcafebabfU, false);
    append_u32(bytes, static_cast<std::uint32_t>(slices.size()), little);
    std::uint64_t offset = 8U + 32U * slices.size();
    for (const auto& [cpu, slice] : slices) {
        append_u32(bytes, cpu, little);
        append_u32(bytes, 0U, little);
        append_u64(bytes, offset, little);
        append_u64(bytes, slice.size(), little);
        append_u32(bytes, 0U, little);
        append_u32(bytes, 0U, little);
        offset += slice.size();
    }
    for (const auto& [cpu, slice] : slices) {
        static_cast<void>(cpu);
        bytes.insert(bytes.end(), slice.begin(), slice.end());
    }
    return bytes;
}

void write(const std::filesystem::path& path, const std::vector<unsigned char>& bytes)
{
    std::ofstream stream(path, std::ios::binary);
    stream.write(
        reinterpret_cast<const char*>(bytes.data()),
        static_cast<std::streamsize>(bytes.size())
    );
    if (!stream) {
        throw std::runtime_error("unable to write fixture");
    }
}
} // namespace

int main(int argc, char** argv)
{
    if (argc != 2) {
        return 2;
    }
    const std::filesystem::path output = argv[1];
    std::filesystem::create_directories(output);
    const auto arm_dylib = thin(cpu_arm64, mh_dylib);
    const auto x86_dylib = thin(cpu_x86_64, mh_dylib);
    const auto arm_executable = thin(cpu_arm64, 2U);

    write(output / "thin-arm64.dylib", arm_dylib);
    write(output / "thin-x86_64.dylib", x86_dylib);
    write(
        output / "universal-arm64-x86_64.dylib",
        fat32(false, {{cpu_arm64, arm_dylib}, {cpu_x86_64, x86_dylib}})
    );
    write(
        output / "universal-little.dylib",
        fat32(true, {{cpu_arm64, arm_dylib}, {cpu_x86_64, x86_dylib}})
    );
    write(
        output / "universal-fat64.dylib",
        fat64(false, {{cpu_arm64, arm_dylib}, {cpu_x86_64, x86_dylib}})
    );
    write(
        output / "universal-fat64-little.dylib",
        fat64(true, {{cpu_arm64, arm_dylib}, {cpu_x86_64, x86_dylib}})
    );
    write(output / "no-arm64.dylib", fat32(false, {{cpu_x86_64, x86_dylib}}));
    write(output / "arm64-wrong-type.dylib", arm_executable);
    write(output / "fat-wrong-type.dylib", fat32(false, {{cpu_arm64, arm_executable}}));

    std::vector<unsigned char> malformed_count;
    append_u32(malformed_count, 0xcafebabeU, false);
    append_u32(malformed_count, 65U, false);
    write(output / "malformed-count.dylib", malformed_count);

    std::vector<unsigned char> truncated_table;
    append_u32(truncated_table, 0xcafebabeU, false);
    append_u32(truncated_table, 2U, false);
    write(output / "truncated-table.dylib", truncated_table);

    std::vector<unsigned char> truncated_slice;
    append_u32(truncated_slice, 0xcafebabeU, false);
    append_u32(truncated_slice, 1U, false);
    append_u32(truncated_slice, cpu_arm64, false);
    append_u32(truncated_slice, 0U, false);
    append_u32(truncated_slice, 28U, false);
    append_u32(truncated_slice, 4U, false);
    append_u32(truncated_slice, 0U, false);
    truncated_slice.insert(truncated_slice.end(), {0xcfU, 0xfaU, 0xedU, 0xfeU});
    write(output / "truncated-slice.dylib", truncated_slice);

    std::vector<unsigned char> overflowing_slice;
    append_u32(overflowing_slice, 0xcafebabeU, false);
    append_u32(overflowing_slice, 1U, false);
    append_u32(overflowing_slice, cpu_arm64, false);
    append_u32(overflowing_slice, 0U, false);
    append_u32(overflowing_slice, 28U, false);
    append_u32(overflowing_slice, 32U, false);
    append_u32(overflowing_slice, 0U, false);
    overflowing_slice.insert(overflowing_slice.end(), 8U, 0U);
    write(output / "overflowing-slice.dylib", overflowing_slice);

    std::vector<unsigned char> slice_inside_table;
    append_u32(slice_inside_table, 0xcafebabeU, false);
    append_u32(slice_inside_table, 1U, false);
    append_u32(slice_inside_table, cpu_arm64, false);
    append_u32(slice_inside_table, 0U, false);
    append_u32(slice_inside_table, 8U, false);
    append_u32(slice_inside_table, 32U, false);
    append_u32(slice_inside_table, 0U, false);
    slice_inside_table.insert(slice_inside_table.end(), 32U, 0U);
    write(output / "slice-inside-table.dylib", slice_inside_table);

    std::vector<unsigned char> overflowing_fat64;
    append_u32(overflowing_fat64, 0xcafebabfU, false);
    append_u32(overflowing_fat64, 1U, false);
    append_u32(overflowing_fat64, cpu_arm64, false);
    append_u32(overflowing_fat64, 0U, false);
    append_u64(overflowing_fat64, 0x8000000000000000ULL, false);
    append_u64(overflowing_fat64, 32U, false);
    append_u32(overflowing_fat64, 0U, false);
    append_u32(overflowing_fat64, 0U, false);
    write(output / "overflowing-fat64.dylib", overflowing_fat64);

    write(output / "truncated-thin.dylib", {0xcfU, 0xfaU, 0xedU, 0xfeU});
    write(output / "static-archive.a", {'!', '<', 'a', 'r', 'c', 'h', '>', '\n'});
    return 0;
}
