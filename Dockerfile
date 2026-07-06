FROM ubuntu:latest
LABEL authors="landrece"

ENTRYPOINT ["top", "-b"]