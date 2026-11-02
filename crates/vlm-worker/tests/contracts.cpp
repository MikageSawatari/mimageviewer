#include "contracts.h"
#include <iostream>

int main() {
    using json = nlohmann::json;
    int failures = 0;
    auto check = [&](bool value) { if (!value) ++failures; };
    json good{{"caption", "説明"}, {"text", ""}, {"keywords", {"写真"}}};
    auto valid = [&](json obj, bool eog = true, int tokens = 100) { return validate_output(obj.dump(), eog, tokens)["ok"].get<bool>(); };
    check(valid(good));
    check(!valid(good, false, 800));
    check(!valid(good, true, 801));
    check(!validate_output("{\"caption\":", true, 10)["ok"].get<bool>());
    check(!validate_output(good.dump() + "garbage", true, 10)["ok"].get<bool>());
    auto bad = good; bad["text"] = nullptr; check(!valid(bad));
    bad = good; bad["keywords"] = {1}; check(!valid(bad));
    bad = good; bad["caption"] = std::string(401, 'a'); check(!valid(bad));
    bad = good; bad["text"] = std::string(401, 'a'); check(!valid(bad));
    bad = good; bad["keywords"] = json::array(); for (int i = 0; i < 21; ++i) bad["keywords"].push_back("a"); check(!valid(bad));
    bad = good; bad["keywords"] = {std::string(33, 'a')}; check(!valid(bad));
    bad = good; bad["keywords"] = {std::string(32, 'a')}; check(valid(bad));
    check(unicode_length("日本😀") == 3);
    std::cout << "contract failures=" << failures << '\n';
    return failures ? 1 : 0;
}
