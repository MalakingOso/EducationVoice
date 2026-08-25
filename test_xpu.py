import torch

print(f"PyTorch version: {torch.__version__}")
print(f"XPU available: {torch.xpu.is_available()}")

if torch.xpu.is_available():
    print(f"XPU device count: {torch.xpu.device_count()}")
    for i in range(torch.xpu.device_count()):
        print(f"  Device {i}: {torch.xpu.get_device_name(i)}")

    # Quick tensor test
    t = torch.randn(3, 3, device="xpu")
    print(f"\nTensor on XPU:\n{t}")
    print(f"Device: {t.device}")
else:
    print("\nXPU not available. Check that:")
    print("  1. torch was installed from the XPU index:")
    print("     pip install torch --index-url https://download.pytorch.org/whl/xpu")
    print("     (the default PyPI wheel has no XPU support)")
    print("  2. Intel GPU drivers / Level Zero runtime are installed")
    print("  3. The GPU is enumerable: run `sycl-ls` and check `lspci`")
