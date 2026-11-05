| Measure | Python | Rust | Rust / Python |
|---|---|---|---|
| `virt-install` time | 529 ± 3 ms | 349 ± 4 ms | 0.66 |
| `virt-install` peak RSS | 124 ± 0 MiB | 84 ± 0 MiB | 0.68 |
| `virt-xml` time | 273 ± 3 ms | 14 ± 0 ms | 0.05 |
| `virt-xml` peak RSS | 74 ± 0 MiB | 27 ± 0 MiB | 0.36 |
| `virt-clone` time | 277 ± 2 ms | 13 ± 0 ms | 0.05 |
| `virt-clone` peak RSS | 76 ± 1 MiB | 27 ± 0 MiB | 0.35 |
| GUI: main window shown | 615 ± 9 ms | 255 ± 4 ms | 0.41 |
| GUI: idle RSS | 126 ± 0 MiB | 159 ± 0 MiB | 1.26 |
| GUI: idle RSS, `GSK_RENDERER=cairo` | | 128 ± 0 MiB | 1.02 |
| GUI: idle CPU | 0.10 ± 0.00 % | 0.00 ± 0.10 % | n/a |

Values are median ± interquartile range. Idle CPU resolution is 0.1%.
