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
    print("  1. intel-extension-for-pytorch is installed")
    print("  2. Intel GPU drivers are up to date")
    print("  3. You installed the correct PyTorch + IPEX versions")
