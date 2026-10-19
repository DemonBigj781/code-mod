#include <iostream>
#include <numeric>
#include <vector>

int main() {
    const std::vector<int> values{10, 12, 20};
    if (std::accumulate(values.begin(), values.end(), 0) != 42) {
        return 1;
    }
    std::cout << "Source-built fork C++ runtime: OK\n";
    return 0;
}
