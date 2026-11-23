FROM python:3.11-slim
WORKDIR /app
ARG PIP_INDEX_URL=https://pypi.org/simple
COPY requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r requirements.txt
COPY server.py identity.py casdoor.py /app/
RUN useradd --uid 10001 --create-home suojian && mkdir /data && chown suojian:suojian /data
USER suojian
EXPOSE 47831
CMD ["python", "-u", "server.py", "--host", "0.0.0.0", "--port", "47831", "--data", "/data"]
