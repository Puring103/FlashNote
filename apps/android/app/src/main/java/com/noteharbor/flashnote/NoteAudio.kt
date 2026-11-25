package com.noteharbor.flashnote

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaPlayer
import android.media.MediaRecorder
import android.os.Build
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.system.Os
import android.system.OsConstants
import android.content.pm.PackageManager
import androidx.core.content.ContextCompat
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.io.File
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

private const val SAMPLE_RATE = 16_000
private const val WAV_HEADER = 44
private const val MAX_PCM_BYTES = SAMPLE_RATE * 2 * 60 * 10

internal class EditorAudioPlayer {
    private var player: MediaPlayer? = null
    var playing by mutableStateOf<String?>(null)
        private set

    fun toggle(directory: File, asset: String) {
        val same = playing == asset
        stop()
        if (same) return
        val next = MediaPlayer()
        try {
            directory.resolve(asset).inputStream().use { next.setDataSource(it.fd) }
            next.setOnCompletionListener { if (player == next) stop() }
            next.setOnErrorListener { _, _, _ -> if (player == next) stop(); true }
            next.prepare()
            next.start()
            player = next
            playing = asset
        } catch (error: Exception) {
            next.release()
            throw error
        }
    }

    fun stop() {
        player?.release()
        player = null
        playing = null
    }
}

internal class NoteRecording @SuppressLint("MissingPermission") constructor(
    context: Context, private val file: File, onFinished: (Result<Unit>) -> Unit,
) {
    private val stopping = AtomicBoolean(false)

    init {
        check(ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) { "需要麦克风权限" }
        val minimum = AudioRecord.getMinBufferSize(SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        check(minimum > 0) { "这台设备无法创建录音" }
        val recorder = AudioRecord(MediaRecorder.AudioSource.MIC, SAMPLE_RATE,
            AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, minimum.coerceAtLeast(8192))
        try {
            check(recorder.state == AudioRecord.STATE_INITIALIZED) { "无法打开麦克风" }
            recorder.startRecording()
            check(recorder.recordingState == AudioRecord.RECORDSTATE_RECORDING) { "无法开始录音" }
        } catch (error: Exception) {
            recorder.release()
            throw error
        }
        thread(name = "FlashNote microphone") {
            val result = runCatching {
                file.parentFile?.mkdirs()
                RandomAccessFile(file, "rw").use { output ->
                    output.setLength(0)
                    output.write(ByteArray(WAV_HEADER))
                    val samples = ShortArray(2048)
                    val pcm = ByteBuffer.allocate(samples.size * 2).order(ByteOrder.LITTLE_ENDIAN)
                    var size = 0
                    while (!stopping.get() && size < MAX_PCM_BYTES) {
                        val count = recorder.read(samples, 0, minOf(samples.size, (MAX_PCM_BYTES - size) / 2))
                        check(count >= 0) { "读取麦克风失败" }
                        pcm.clear()
                        repeat(count) { pcm.putShort(samples[it]) }
                        output.write(pcm.array(), 0, count * 2)
                        size += count * 2
                    }
                    check(size > 0) { "没有录到声音" }
                    output.seek(0)
                    output.write(wavHeader(size))
                    output.fd.sync()
                }
            }
            runCatching { recorder.stop() }
            recorder.release()
            if (result.isFailure) file.delete()
            onFinished(result)
        }
    }

    fun stop() { stopping.set(true) }
}

private fun wavHeader(size: Int): ByteArray = ByteBuffer.allocate(WAV_HEADER).order(ByteOrder.LITTLE_ENDIAN)
    .put("RIFF".toByteArray()).putInt(size + 36).put("WAVEfmt ".toByteArray()).putInt(16)
    .putShort(1).putShort(1).putInt(SAMPLE_RATE).putInt(SAMPLE_RATE * 2).putShort(2).putShort(16)
    .put("data".toByteArray()).putInt(size).array()

internal fun canTranscribe(context: Context): Boolean = Build.VERSION.SDK_INT >= 33 &&
    SpeechRecognizer.isOnDeviceRecognitionAvailable(context)

internal suspend fun transcribe(context: Context, file: File): String = withContext(Dispatchers.Main) {
    if (Build.VERSION.SDK_INT < 33 || !canTranscribe(context)) {
        error("这台设备没有可用的离线语音识别服务")
    }
    val recognizer = SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
    try {
        ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY).use { audio ->
            Os.lseek(audio.fileDescriptor, WAV_HEADER.toLong(), OsConstants.SEEK_SET)
            val request = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
                .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE, audio)
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_CHANNEL_COUNT, 1)
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_ENCODING, AudioFormat.ENCODING_PCM_16BIT)
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_SAMPLING_RATE, SAMPLE_RATE)
            withTimeoutOrNull(120_000) {
                suspendCancellableCoroutine { continuation ->
                    recognizer.setRecognitionListener(object : RecognitionListener {
                        override fun onResults(results: Bundle) {
                            val text = results.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()?.trim()
                            if (!continuation.isActive) return
                            if (text.isNullOrEmpty()) continuation.resumeWithException(IllegalStateException("未识别到文字"))
                            else continuation.resume(text)
                        }
                        override fun onError(error: Int) {
                            if (continuation.isActive) continuation.resumeWithException(IllegalStateException("离线转写失败（$error），请检查手机的语音识别服务和离线语言包"))
                        }
                        override fun onReadyForSpeech(params: Bundle?) = Unit
                        override fun onBeginningOfSpeech() = Unit
                        override fun onRmsChanged(rmsdB: Float) = Unit
                        override fun onBufferReceived(buffer: ByteArray?) = Unit
                        override fun onEndOfSpeech() = Unit
                        override fun onPartialResults(partialResults: Bundle?) = Unit
                        override fun onEvent(eventType: Int, params: Bundle?) = Unit
                    })
                    recognizer.startListening(request)
                }
            } ?: error("离线转写超时")
        }
    } finally {
        recognizer.destroy()
    }
}
