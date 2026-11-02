# 9.0 runs on Python 2, which reads a tab as indentation to the next multiple of 8.


def total(lines):
    amount = 0
    for line in lines:
	amount += line
    return amount
