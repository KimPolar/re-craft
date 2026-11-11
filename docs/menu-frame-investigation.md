# Menu frame pacing investigation

The title-screen window can accept redraw requests regularly while its GPU
worker waits for a presentable surface image. CPU redraw-request timing is not
the same measurement as actual display scanout, and frame submission completion
does not establish when a frame appears on the monitor.

On one Windows test environment, 1920×1080, preloaded 26.3 assets, a reported
144 Hz display, and the Intel Arc 140T Vulkan adapter, the 14-second title-window
test produced these CPU-side measurements. Each GPU stage uses 300 samples
after the existing warmup. DirectX 12 adapter initialization failed in this
environment, so these results do not describe that backend or other machines.

| Setting | Surface latency limit | Render-interval median | Surface acquisition median |
| --- | ---: | ---: | ---: |
| VSync on | 2 (default) | 31.27 ms | 30.08 ms |
| VSync on | 1 | 31.17 ms | 29.95 ms |
| VSync on | 3 | 31.33 ms | 30.14 ms |
| VSync off | 2 (default) | 16.66 ms | 0.12 ms |

The UI redraw-request median was approximately 16.6 ms in these runs. The
VSync results suggest the acquisition wait is a significant contributor in
this environment; they do not establish why the driver or compositor waits, or
prove the cause of another user's stutter. The available Vulkan present modes
reported by this surface were FIFO and immediate. Changing the latency limit
did not show an improvement, so production settings were not changed. A
separate 1280×720 offscreen panorama-and-overlay test had a 0.99 ms median
submit-and-complete time; its size and lack of presentation mean it is not a
direct comparison with the window measurements.

The following PowerShell commands run the diagnostic in the test executable;
the diagnostic controls are not included in a normal release build:

```powershell
$env:NATIVE_MENU_TEST_1080 = '1'
$env:NATIVE_MENU_TEST_LONG = '1'
$env:NATIVE_MENU_FRAME_TIMINGS = '1'
cargo test -p native-client --bin native-client menu::tests::measure_title_window_pacing --locked -- --ignored --nocapture
```

Set `NATIVE_MENU_TEST_NO_VSYNC` to test without VSync, or set
`NATIVE_MENU_TEST_FRAME_LATENCY` to compare a swapchain latency limit. Clear
those variables between comparisons. The test preloads assets, so it does not
measure installation detection or initial resource loading.

Next, compare DirectX 12 and Vulkan on the user's normal desktop and compare
Java 26.3 under the same window, refresh-rate and VSync conditions. A display
presentation trace is needed to distinguish compositor/driver pacing from
missed application frames. Keep VSync behavior and the Java-compatible UI
unchanged until an improvement is demonstrated under comparable conditions.
