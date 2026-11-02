"""Portable names for exact vendor manifests; leave file bytes untouched."""


def index_vendor_files(package):
    return {p.relative_to(package).as_posix(): p for p in package.rglob("*") if p.is_file()}
