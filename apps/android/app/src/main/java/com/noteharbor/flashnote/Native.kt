package com.noteharbor.flashnote

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

object Native {
 init { System.loadLibrary("suojian_android") }
 @JvmStatic external fun request(root: String, request: String): String
 @JvmStatic fun protect(input: ByteArray, decrypt: Boolean): ByteArray {
  val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
  val alias = "suojian.local.v1"
  if (!store.containsAlias(alias)) {
   check(!decrypt) { "设备密钥丢失" }
   KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
    init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
     .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
    generateKey()
   }
  }
  val key = store.getKey(alias, null) as SecretKey
  val cipher = Cipher.getInstance("AES/GCM/NoPadding")
  return if (decrypt) {
   require(input.size >= 29 && input[0] == 1.toByte())
   cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, input.copyOfRange(1,13)))
   cipher.doFinal(input,13,input.size-13)
  } else {
   cipher.init(Cipher.ENCRYPT_MODE,key)
   byteArrayOf(1) + cipher.iv + cipher.doFinal(input)
  }
 }
}
