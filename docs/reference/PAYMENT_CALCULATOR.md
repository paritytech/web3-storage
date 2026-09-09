# Storage Agreement Payment Calculator

Quick reference for calculating payment amounts when creating storage agreements.

## Payment Formula

```
payment = price_per_byte × max_bytes × duration
```

`duration` is measured in **anchor (relay-chain) blocks** — 6 s each — not
parachain blocks, so agreement costs keep their wall-clock meaning if the
parachain block time changes.

## Common Values

### Token Decimals
- 1 token = 1,000,000,000,000 (12 decimals)
- 1 milltoken = 1,000,000,000
- 1 microtoken = 1,000,000

### Storage Sizes
- 1 KB = 1,024 bytes
- 1 MB = 1,048,576 bytes
- 1 GB = 1,073,741,824 bytes
- 10 GB = 10,737,418,240 bytes

### Duration (anchor blocks, 6 seconds each)
- 1 hour ≈ 600 blocks
- 1 day ≈ 14,400 blocks
- 1 week ≈ 100,800 blocks
- 1 month ≈ 432,000 blocks

## Examples

### Example 1: 1 GB for 500 blocks

**Given:**
- Price per byte: 1,000,000 (1 microtoken)
- Storage size: 1 GB = 1,073,741,824 bytes
- Duration: 500 blocks (~50 minutes)

**Calculation:**
```
payment = 1,000,000 × 1,073,741,824 × 500
payment = 536,870,912,000,000,000
```

**With 10% buffer:**
```
maxPayment = 536,870,912,000,000,000 × 1.1
maxPayment = 590,558,003,200,000,000
```

**With 20% buffer:**
```
maxPayment = 536,870,912,000,000,000 × 1.2
maxPayment = 644,245,094,400,000,000
```

---

### Example 2: 10 GB for 1 week

**Given:**
- Price per byte: 1,000,000 (1 microtoken)
- Storage size: 10 GB = 10,737,418,240 bytes
- Duration: 100,800 blocks (1 week)

**Calculation:**
```
payment = 1,000,000 × 10,737,418,240 × 100,800
payment = 1,082,331,518,592,000,000,000
```

**With 10% buffer:**
```
maxPayment = 1,190,564,670,451,200,000,000
```

---

### Example 3: 100 MB for 1 day

**Given:**
- Price per byte: 500,000 (0.5 microtoken - cheaper provider)
- Storage size: 100 MB = 104,857,600 bytes
- Duration: 14,400 blocks (1 day)

**Calculation:**
```
payment = 500,000 × 104,857,600 × 14,400
payment = 754,790,400,000,000,000
```

**With 10% buffer:**
```
maxPayment = 830,269,440,000,000,000
```

---

## Quick Reference Table

Common configurations with 10% buffer:

| Size | Duration | Price/Byte | Payment | maxPayment (10%) |
|------|----------|------------|---------|------------------|
| 1 GB | 500 blocks | 1,000,000 | 536,870,912,000,000,000 | 590,558,003,200,000,000 |
| 1 GB | 1 day | 1,000,000 | 15,461,882,265,600,000,000 | 17,008,070,492,160,000,000 |
| 1 GB | 1 week | 1,000,000 | 108,233,151,859,200,000,000 | 119,056,467,045,120,000,000 |
| 10 GB | 1 day | 1,000,000 | 154,618,822,656,000,000,000 | 170,080,704,921,600,000,000 |
| 100 MB | 1 day | 1,000,000 | 1,509,949,440,000,000,000 | 1,660,944,384,000,000,000 |

---

## Calculator Steps

1. **Check provider's price per byte:**
   ```
   Query: storageProvider.providers(PROVIDER_ACCOUNT)
   Look at: settings.pricePerByte
   ```

2. **Determine your storage size in bytes:**
   - 1 GB = 1,073,741,824 bytes
   - 10 GB = 10,737,418,240 bytes

3. **Choose duration in blocks:**
   - 1 hour ≈ 600 blocks
   - 1 day ≈ 14,400 blocks
   - 1 week ≈ 100,800 blocks

4. **Calculate payment:**
   ```
   payment = price_per_byte × max_bytes × duration
   ```

5. **Add buffer (10-20%):**
   ```
   maxPayment = payment × 1.1   (10% buffer)
   maxPayment = payment × 1.2   (20% buffer)
   ```

6. **Use in extrinsic:**
   ```
   requestPrimaryAgreement(
       bucketId,
       provider,
       max_bytes,
       duration,
       maxPayment  ← Your calculated value
   )
   ```

---

## Why Use a Buffer?

The `maxPayment` parameter is a safety mechanism:

1. **Price Changes**: Provider might increase prices between request and acceptance
2. **Duration Adjustments**: Slight variations in block times
3. **Safety Margin**: Prevents transactions from failing due to minor calculation differences

**Recommended:** Use 10-20% buffer for most cases.

---

## JavaScript Calculator

```javascript
// Helper function to calculate payment
function calculatePayment(pricePerByte, maxBytes, duration) {
    return BigInt(pricePerByte) * BigInt(maxBytes) * BigInt(duration);
}

// Example usage
const pricePerByte = 1_000_000;        // 1 microtoken
const maxBytes = 1_073_741_824;        // 1 GB
const duration = 500;                   // blocks

const payment = calculatePayment(pricePerByte, maxBytes, duration);
const maxPayment = payment * 11n / 10n; // 10% buffer

console.log(`Payment: ${payment}`);
console.log(`maxPayment (with 10% buffer): ${maxPayment}`);

// Output:
// Payment: 536870912000000000
// maxPayment (with 10% buffer): 590558003200000000
```

---

## Python Calculator

```python
def calculate_payment(price_per_byte, max_bytes, duration):
    return price_per_byte * max_bytes * duration

# Example usage
price_per_byte = 1_000_000        # 1 microtoken
max_bytes = 1_073_741_824         # 1 GB
duration = 500                     # blocks

payment = calculate_payment(price_per_byte, max_bytes, duration)
max_payment = int(payment * 1.1)   # 10% buffer

print(f"Payment: {payment}")
print(f"maxPayment (with 10% buffer): {max_payment}")

# Output:
# Payment: 536870912000000000
# maxPayment (with 10% buffer): 590558003200000000
```

---

## Common Mistakes

❌ **Forgetting maxPayment parameter**
```
requestPrimaryAgreement(0, provider, 1073741824, 500)
// Missing maxPayment!
```

✅ **Correct usage**
```
requestPrimaryAgreement(0, provider, 1073741824, 500, 600000000000000000)
// Includes maxPayment
```

❌ **Setting maxPayment too low**
```
maxPayment: 100000000000000  // Way too low!
// Error: PaymentExceedsMax
```

✅ **Proper maxPayment with buffer**
```
maxPayment: 590558003200000000  // Calculated + 10% buffer
// Transaction succeeds
```

❌ **Wrong decimal places**
```
maxPayment: 536870912  // Missing 9 zeros!
// Error: PaymentExceedsMax
```

✅ **Correct decimal places (12 decimals)**
```
maxPayment: 536870912000000000  // Correct!
```
