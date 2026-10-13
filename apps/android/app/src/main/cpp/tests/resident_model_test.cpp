#include "../resident_model.h"
#include <cassert>
#include <cstdlib>
#include <fstream>
#include <filesystem>

struct FakeModel { int generation; };
static int loads = 0, frees = 0;
static void destroy(FakeModel * model) { ++frees; delete model; }

int main() {
    char pattern[] = "/tmp/writer-resident-model-XXXXXX";
    const auto directory = mkdtemp(pattern);
    assert(directory);
    const auto path = std::string(directory) + "/model.bin";
    const auto other = std::string(directory) + "/other.bin";
    std::ofstream(path) << "fake weights";
    std::ofstream(other) << "other weights";
    auto load = [](const char *) { ++loads; assert(frees == loads - 1); return new FakeModel{loads}; };
    {
        ResidentModel<FakeModel, destroy> cache;
        assert(cache.get(path, load)->generation == 1);
        assert(cache.get(path, load)->generation == 1);
        assert(loads == 1 && frees == 0);
        std::filesystem::rename(other, path); // Replacement at the identical path must reload.
        assert(cache.get(path, load)->generation == 2);
        cache.clear(); cache.clear();
        assert(frees == 2);
        assert(cache.get(path, load)->generation == 3);
        std::filesystem::remove(path);
        bool failed = false;
        try { cache.get(path, load); } catch (const std::runtime_error &) { failed = true; }
        assert(failed && frees == 3);
        std::ofstream(path) << "new weights";
        failed = false;
        try { cache.get(path, [](const char *) -> FakeModel * { return nullptr; }); }
        catch (const std::runtime_error &) { failed = true; }
        assert(failed);
        assert(cache.get(path, load)->generation == 4);
    }
    assert(loads == 4 && frees == 4);
    std::filesystem::remove_all(directory);
}
