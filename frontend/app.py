import os
import sys
import click
from datetime import datetime, timedelta
import pytz

from flask import Flask, render_template, jsonify, request
from flask_sqlalchemy import SQLAlchemy
from sqlalchemy import func

if sys.platform.startswith('win'):
    prefix = 'sqlite:///'
else:
    prefix = 'sqlite:////'

app = Flask(__name__)
app.config['SQLALCHEMY_DATABASE_URI'] = prefix + os.path.join(app.root_path, 'data.db')
app.config['SQLALCHEMY_TRACK_MODIFICATIONS'] = False

db = SQLAlchemy(app)

class Record(db.Model):
    time = db.Column(db.DateTime, primary_key=True)
    temperature = db.Column(db.Float)
    humidity = db.Column(db.Float)

@app.cli.command()  # 注册为命令，可以传入 name 参数来自定义命令
@click.option('--drop', is_flag=True, help='Drop existing tables')  # 设置选项
def initdb(drop):
    """Initialize the database."""
    if drop:
        db.drop_all()
    db.create_all()
    click.echo('Setting up database completed.')

@app.cli.command()
def forge():
    db.create_all()
    current = datetime.now()
    for i in range(100):
        record = Record(time=current, temperature=float(i), humidity=float(i))
        current = current + timedelta(minutes=1)
        db.session.add(record)
    db.session.commit()
    click.echo('Forging database completed.')

@app.route('/data')
def data():
    DEFAULT_LIMIT = -1
    limit = request.args.get('limit', DEFAULT_LIMIT, type=int)
    if limit <= 0:
        records = Record.query.all()
    else:
        records = Record.query.order_by(Record.time.desc()).limit(limit).all()
        records.reverse()
    dataset = []
    for record in records:
        dataset.append({
            "time": record.time.strftime('%Y-%m-%d %H:%M:%S'),
            "temperature": record.temperature,
            "humidity": record.humidity,
        })
    return jsonify(dataset)

@app.route('/')
def index():
    return render_template('index.html')

@app.route('/append', methods=["GET", "PUT"])
def append():
    MIN_VALUE = -300
    temperature = request.args.get('temperature', MIN_VALUE, type=float)
    humidity = request.args.get('humidity', MIN_VALUE, type=float)
    if temperature > MIN_VALUE and humidity > MIN_VALUE:
        record = Record(time=datetime.now(), temperature=temperature, humidity=humidity)
        db.session.add(record)
        db.session.commit()
        return jsonify({ "status": "ok" })
    else:
        return jsonify({ "status": "invalid_argument" })
