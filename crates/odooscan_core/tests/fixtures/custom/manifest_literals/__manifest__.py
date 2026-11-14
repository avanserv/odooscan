# Every form here is a literal ast.literal_eval reads.
{
    "name": "Manifest " "literals",  # implicit concatenation
    "version": "18.0.1.0.0",
    "sequence": -1,
    "depends": ["base"],
    "external_dependencies": {"python": set()},
    "x_pairs": [{("a", 1), ("b", 2)}],  # a set of hashable tuples
    "installable": True,
    "license": "LGPL-3",
}
