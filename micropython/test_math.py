print("=== MicroPython Integer Math Test ===")
values = [1, 2, 3, 4, 5]
sum_squares = 0
for value in values:
    sum_squares += value * value

print("sum_squares =", sum_squares)
print("7 * 8 + 3 =", 7 * 8 + 3)
print("divmod(23, 5) =", divmod(23, 5))
print("=== MicroPython Integer Math Test Complete ===")
